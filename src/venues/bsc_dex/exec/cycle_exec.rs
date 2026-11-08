//! Файл: venues/bsc_dex/exec/cycle_exec.rs
//! Что делает: сделка цикла DEX ↔ DEX ОДНОЙ транзакцией через свой исполнитель ArbExecutor (решения 122 X, 124) —
//! предпочтительная форма: атомарна сама (не зависит от бандл-каналов), без пыли межроутерного перехода, любые виды
//! шагов исполнителя. Шаги — exec/cycle_steps.rs (одна доля: вход у кошелька → … → базовая кошельку, minOut — граница
//! ядра). Порядок, как у ноги через исполнитель (exec/executor/trade.rs): место в очереди кошелька (решение 45) → код
//! и владелец исполнителя сверяются с узлом (executor/onchain.rs) → разрешение ERC-20 базовой исполнителю (один раз на
//! токен; отправлено — ждало блока, пулы перечитываются и всё собирается заново, новое разрешение после этого — отказ,
//! решение 121 B1) → проба execute eth_simulateV1 поверх блока сборки (откат — сделки нет; received ≥ minOut) →
//! чистая прибыль по газу симуляции ≥ dex_dex.min_profit_usd → одна транзакция существующими каналами (exec/txs.rs) →
//! факт по событиям Transfer базовой монеты кошелька (exec/result.rs: ушло — вход, пришло — выход). Повторов нет.
//! Разбор exec/executor/fact.rs («возврат неистраченного входа») к циклу не применяется: вход и выход — один токен.
//! Исход транзакции цикла неизвестен — «исход неизвестен» с «чем найти» цикла (exec/cycle_ref.rs, решение 233,
//! дополнение, п. 4): ядро выясняет исход само (resolve_cycle.rs).
//! Кто вызывает: exec/cycle_trade.rs.
//! От чего зависит: exec/executor/{call, onchain, sim}.rs, exec/{cycle_steps, cycle_ref, swap_send, txs, result}.rs,
//! quote/{cycle_legs, cycle_cost}.rs, wallet/{approve_executor, nonce}.rs, check/simulate.rs.
//! Настройки: exec.executor_address; через вызываемые — dex.deadline_s, dex.approve_unlimited, verify.*, send.*,
//! channel.*, dex_dex.min_profit_usd.
//! Уведомление: dex.rebuilt_after_approve (как у ноги).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{CycleTradeRequest, CycleTradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::simulate::{self, Base, Call};
use super::super::quote::cycle_cost::Rates;
use super::super::quote::route_calc::Leg;
use super::super::quote::{Quoting, cycle_legs};
use super::super::wallet::approve_executor;
use super::super::wallet::nonce::Nonces;
use super::cycle_ref;
use super::cycle_steps::{self, exec_steps, net_check};
use super::executor::call::{self, ExecCall};
use super::executor::{onchain, sim};
use super::result;
use super::swap_send::{rebuilt_notice, send_approvals};
use super::txs::{self, Env};

/// Сделка через исполнитель. Принимает: котировку, окружение отправки, очередь кошелька, запрос, свежие ноги, адрес
/// исполнителя, курсы. Ошибки: NoRoute (проба откатилась), QuoteMismatch (ниже границы, газ съел прибыль, новое
/// разрешение), StaleData, InvalidAddress (исполнитель не наш), настройки, каналы, узел, SwapReverted / TxNotIncluded /
/// OutcomeUnknown (с фактом транзакции).
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: &CycleTradeRequest,
    legs: Vec<Leg>,
    (executor, r): (Address, Rates),
) -> GatewayResult<CycleTradeResult> {
    let ctx = &q.ctx;
    let dec = legs[0].dec_in;
    let base = req.cycle.base;
    let tr = cycle_steps::trade_request(req);
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    onchain::ensure(ctx, executor, wallet).await?;
    let (mut c, mut at) = build(q, &legs, req, executor).await?;
    let need = approve_executor::missing(ctx, base, wallet, executor, c.amount_in).await?;
    if send_approvals(env, &slot, &need).await? {
        // Решение 121 B1: разрешение ждало блока — пулы, выход и вызов устарели: свежее перечтение и сборка.
        let (legs, _) = cycle_legs::fresh(q, &req.cycle).await?;
        (c, at) = build(q, &legs, req, executor).await?;
        if !approve_executor::missing(ctx, base, wallet, executor, c.amount_in)
            .await?
            .is_empty()
        {
            let why = "после approve свежий цикл требует нового разрешения исполнителю — своп не отправлен";
            return Err(ctx.err(ErrorCode::QuoteMismatch, why));
        }
        rebuilt_notice(env, &tr);
    }
    let (got, gas) = probe(ctx, &c, wallet, at).await?;
    let out = super::super::num::to_decimal(got, dec)
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "выход пробы не в Decimal"))?;
    net_check(ctx, (out, req.input), (gas, cycle_steps::wei(q).await?), &r)?;
    // Решение 233 (дополнение, п. 4): исход неизвестен — «чем найти» цикла (контекст свопа «базовая → базовая» и курсы
    // сделки), позднее выяснение своим узлом теми же функциями (resolve_cycle.rs). Разрешения выше — без него: их
    // «исход неизвестен» несёт отметку approve (exec/swap_send.rs), обмен после них не отправлялся.
    let sc = cycle_ref::ctx(wallet, base, dec, &r);
    let d = txs::send(env, &slot, executor, c.data(), U256::ZERO, true, None)
        .await
        .map_err(|e| sc.attach(e))?;
    drop(slot);
    let t = result::result(ctx, tr, d, wallet, (base, base), (dec, dec), &[])
        .map_err(|e| sc.attach(e))?;
    Ok(cycle_steps::finish(t, &r))
}

/// Вызов execute на текущем блоке: ожидаемый выход на этих ногах не ниже границы ядра. Возвращает: вызов и (номер,
/// время) блока. Ошибки: QuoteMismatch (ниже границы), как call::build, узел.
pub async fn build(
    q: &Quoting,
    legs: &[Leg],
    req: &CycleTradeRequest,
    executor: Address,
) -> GatewayResult<(ExecCall, (u64, u64))> {
    let ctx = &q.ctx;
    let (steps, expected) = exec_steps(ctx, legs, req.input, req.min_out)?;
    below(ctx, expected, req.min_out)?;
    let (at, now) = q.heads.at(ctx).await?;
    Ok((call::build(ctx, &[steps], executor, now)?, (at, now)))
}

/// Ожидаемый выход ниже границы ядра — сделки нет. Ошибки: QuoteMismatch.
pub fn below(
    ctx: &super::super::ctx::Ctx,
    expected: Decimal,
    min_out: Decimal,
) -> GatewayResult<()> {
    if expected < min_out {
        let why = format!(
            "на свежем состоянии пулов выход цикла {expected} ниже границы {min_out} — сделки нет"
        );
        return Err(ctx.err(ErrorCode::QuoteMismatch, why));
    }
    Ok(())
}

/// Проба execute eth_simulateV1 поверх блока сборки (время головы + 1 с). Возвращает: сколько получит кошелёк
/// (= весь выход цикла) и газ. Ошибки: NoRoute (откат, в т. ч. «minOut»), StaleData (срок), BadResponse, узел.
async fn probe(
    ctx: &super::super::ctx::Ctx,
    c: &ExecCall,
    wallet: Address,
    (at, now): (u64, u64),
) -> GatewayResult<(U256, u64)> {
    let call = Call {
        from: wallet,
        to: c.executor,
        input: c.data(),
    };
    let res = simulate::run(ctx, &[call], Base::timed(at, now)).await?;
    let r = res
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "eth_simulateV1: нет итога вызова"))?;
    let got = sim::received(ctx, r)?;
    if got < c.min_out {
        let why = format!("проба исполнителя: получено {got} < minOut {}", c.min_out);
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    Ok((got, r.gas_used.to::<u64>()))
}
