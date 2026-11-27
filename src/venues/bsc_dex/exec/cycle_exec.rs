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
//! Решение 273: перед отправкой — пауза и предел цены газа фабрик пар Solidly заново (exec/solidly_gate.rs fresh_cap),
//! предел — по цене самой транзакции (exec/txs.rs send_capped).
//! Решение 437 (2): газ в проверке прибыли — exec/cycle_gas.rs (у eip1559_l1 чаевые только с газа L2, часть L1 —
//! gasEstimateComponents этой транзакции, запрос одновременно с пробой).
//! Решение 437 (3): send.tip_mode = profit_share — чаевые от прибыли цикла (exec/tip_cycle.rs choose), отправка
//! txs::send_tipped, итог — доле чаевых в фоне (tip_cycle::after).
//! Решение 438: knownAccounts пулов маршрута (блок плана, exec/pool_cond_read.rs) — в условную отправку секвенсора;
//! пулы в условии и без него — журнал send.pool_cond (в фоне), TradeResult.executor и текст ошибки после отправки.
//! Решение 439: итог отправки — в статистику условия пулов по режиму в действии (exec/pool_cond_stat.rs, в фоне).
//! Кто вызывает: exec/cycle_trade.rs.
//! От чего зависит: exec/executor/{call, callback_at, onchain, pool_pass, sim}.rs, exec/{cycle_steps, cycle_ref, swap_send, txs, result,
//! pool_cond_read, pool_cond_row, pool_cond_stat}.rs,
//! quote/{cycle_legs, cycle_cost}.rs, wallet/{approve_executor, nonce}.rs, check/simulate.rs.
//! Настройки: exec.executor_address; через вызываемые — dex.deadline_s, dex.approve_unlimited, verify.*, send.*,
//! channel.*, dex_dex.min_profit_usd.
//! Уведомление: dex.rebuilt_after_approve (как у ноги).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{CycleTradeRequest, CycleTradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::check::simulate::{self, Base, Call};
use super::super::quote::Quoting;
use super::super::quote::cycle_cost::Rates;
use super::super::quote::route_calc::Leg;
use super::super::wallet::approve_executor;
use super::super::wallet::nonce::Nonces;
use super::cycle_gas;
use super::cycle_ref;
use super::cycle_steps::{self, exec_steps};
use super::executor::call::{self, ExecCall};
use super::executor::{onchain, pool_pass, sim};
use super::pool_cond_read;
use super::pool_cond_row::{self, PoolConds};
use super::pool_cond_stat;
use super::result;
use super::swap_send::{rebuilt_notice, send_approvals, with_approve_gas};
use super::tip_cycle;
use super::txs::{self, Env};

/// Сделка через исполнитель. Принимает: котировку, окружение отправки, очередь кошелька, запрос, свежие ноги, (адрес
/// исполнителя, курсы, условие пулов на блоке ног — решение 438; None — без условия пулов). Ошибки: NoRoute (проба
/// откатилась), QuoteMismatch (ниже границы, газ съел прибыль, новое разрешение), StaleData, InvalidAddress
/// (исполнитель не наш), настройки, каналы, узел, SwapReverted / TxNotIncluded / OutcomeUnknown (с фактом транзакции).
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: &CycleTradeRequest,
    legs: Vec<Leg>,
    (executor, r, mut conds): (Address, Rates, Option<PoolConds>),
) -> GatewayResult<CycleTradeResult> {
    let ctx = &q.ctx;
    let dec = legs[0].dec_in;
    let base = req.cycle.base;
    let tr = cycle_steps::trade_request(req);
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let version = onchain::ensure(ctx, executor, wallet).await?;
    // Ревью t181 (Н4): вид шага / обратный вызов пулов цикла — по версии, которую вернула сверка кода в сети (как на
    // пути сделки, exec/executor/trade.rs; задача t122): запись кошелька могла отстать — без пробы с откатом «kind».
    if let Some(why) = legs
        .iter()
        .find_map(|l| super::executor::callback_at::missing_at(ctx, l.entry.family, version))
    {
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    let vista_generation=legs.iter().any(|s|super::vista_gate::needs(&s.entry))
        .then(|| ctx.settings.generation());
    let (mut c, mut at) = build(q, &legs, req, executor).await?;
    let need = approve_executor::missing(ctx, base, wallet, executor, c.amount_in).await?;
    if let Some(generation)=vista_generation && !need.is_empty() {
        Box::pin(super::vista_gate::check(ctx,legs.iter().map(|s|(&s.entry,s.token_in)).collect(),generation)).await?;
    }
    let approved = send_approvals(env, &slot, &need).await?;
    // Решение 236, дополнение (техлид 09.11): газ разрешений — в итог цикла и в любую ошибку после них (swap_send.rs).
    let out: GatewayResult<CycleTradeResult> = async {
    if approved.is_some() {
        // Решение 121 B1: разрешение ждало блока — пулы, выход и вызов устарели: свежее перечтение и сборка.
        // Решение 438: условие пулов — заново на блоке нового перечтения (старые значения — с прошлого плана).
        let (legs, _, renewed) = pool_cond_read::fresh(q, &req.cycle, conds.is_some()).await?;
        conds = renewed;
        (c, at) = build(q, &legs, req, executor).await?;
        if !approve_executor::missing(ctx, base, wallet, executor, c.amount_in)
            .await?
            .is_empty()
        {
            let why = "после approve свежий цикл требует нового разрешения исполнителю — своп не отправлен";
            return Err(ctx.err(ErrorCode::QuoteMismatch, why));
        }
        rebuilt_notice(env, &tr, at.0);
    }
    // Решение 437 (2): цена газа (у eip1559_l1 — с частью L1 этой транзакции, NodeInterface) — одновременно с пробой.
    let mut fee_quote = q.clone();
    fee_quote.gas = q.gas.for_quote(ctx,&q.heads).await?;
    let call = (executor, c.data());
    let ((got, gas), bill) = tokio::try_join!(
        probe(ctx, &c, wallet, at),
        cycle_gas::priced(&fee_quote, wallet, call)
    )?;
    let out = super::super::num::to_decimal(got, dec)
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "выход пробы не в Decimal"))?;
    // Решение 437 (3): profit_share — чаевые сделки от прибыли цикла (exec/tip_cycle.rs), до проверки порога.
    let mut bill = cycle_gas::simulated(&fee_quote, bill, gas, 0, 0)?;
    if let (cycle_gas::Bill::Type2 { extra, .. }, Some(paid)) = (&mut bill, approved) {
        let paid = super::super::num::to_units(paid,super::super::num::BNB_DECIMALS)
            .ok_or_else(|| ctx.err(ErrorCode::BadResponse,"комиссия подтверждённых approve не помещается в wei"))?;
        *extra = extra.saturating_add(paid);
    }
    // Решение 438: knownAccounts — только непустые; ревью 439 (4) — по ним же фильтр разбора проигрыша (tip_cycle).
    let known = conds.as_ref().map(|k| &k.known).filter(|k| !k.is_empty());
    let (bill, tips) = tip_cycle::choose(q, bill, (out, req.input), &r, &legs, known).await?;
    // Исполнитель — одна транзакция с чаевыми (txs::send ниже): чаевые и газ их перевода — в проверке.
    let (_,r,floor)=cycle_steps::funded_net(q,base,(out,req.input),&bill,r,true).await?;
    if crate::settings::cycle_cash::read(&ctx.settings,ctx.place.id).map_err(|why|ctx.err(ErrorCode::NotConfigured,why))?
        .is_some_and(|p|p.token==base && Some(base)!=q.tokens.usdt(ctx).ok()) {
        let minimum=super::super::quote::direct::ceil_units(floor,dec).ok_or_else(||ctx.err(ErrorCode::BadResponse,"Минимальная выплата USDT вне пределов"))?;
        c.min_out=c.min_out.max(minimum);
        if got<c.min_out{return Err(ctx.err(ErrorCode::QuoteMismatch,"Результат симуляции ниже окончательного minOut с газом"));}
    }
    // Решение 233 (дополнение, п. 4): исход неизвестен — «чем найти» цикла (контекст свопа «базовая → базовая» и курсы
    // сделки), позднее выяснение своим узлом теми же функциями (resolve_cycle.rs). Разрешения выше — без него: их
    // «исход неизвестен» несёт отметку approve (exec/swap_send.rs), обмен после них не отправлялся.
    let sc = cycle_ref::ctx(wallet, base, dec, &r);
    // Решение 273: пары Solidly с паузой/пределом газа фабрики — заново с фабрики; предел проверяет send_capped.
    let used: Vec<_> = legs.iter().map(|l| &l.entry).collect();
    let cap = super::solidly_gate::fresh_cap(q, &used).await?;
    if let Some(generation)=vista_generation {
        Box::pin(super::vista_gate::check(ctx,legs.iter().map(|s|(&s.entry,s.token_in)).collect(),generation)).await?;
    }
    let gas_tip = tips.as_ref().map(|t| t.tip);
    // Решение 438: какие пулы в условии — в журнал до отправки (в фоне).
    let note = conds.as_ref().map(|k| pool_cond_row::log(ctx, &req.deal_id, k));
    // Решение 439: режим для статистики — до отправки (решение 398); итог — в фоне после неё. Ревью 439 (5): пустое
    // условие (ни один пул не попал) — отправка без условия пулов, в статистике off.
    let stat = pool_cond_stat::begin(ctx, known.is_some());
    let res = txs::send_tipped(
        env,
        &slot,
        (executor, c.data(), U256::ZERO),
        true,
        None,
        cap,
        (gas_tip, known),
    )
    .await;
    // Решение 437 (3): итог — для доли чаевых, в фоне (сделка не ждёт разбора); решение 438 — с блоком плана.
    let plan_block = conds.as_ref().filter(|k| !k.known.is_empty()).map(|k| k.block);
    tip_cycle::after(env, tips, &res, plan_block);
    pool_cond_stat::after(ctx, stat, &req.deal_id, &res);
    let with_note = |mut e: GatewayError| {
        if let Some(n) = &note {
            e.message = format!("{}; {n}", e.message);
        }
        sc.attach(e)
    };
    let d = res.map_err(with_note)?;
    drop(slot);
    let mut t = result::result(ctx, tr, d, wallet, (base, base), (dec, dec), &[])
        .map_err(with_note)?;
    if let Some(n) = note {
        t.executor = Some(n);
    }
    Ok(cycle_steps::finish(t, &r))
    }
    .await;
    with_approve_gas(out, approved)
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
    let res = simulate::run(ctx, &[call], Base::for_chain(ctx.chain_id(), at, now)).await?;
    let r = res
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "eth_simulateV1: нет итога вызова"))?;
    // Решение 401: откат (не срок, не minOut) помечает пулы шагов менеджера «отвергнут» до перепроверки.
    pool_pass::on_sim(ctx.place.id, c, r);
    let got = sim::received(ctx, r)?;
    if got < c.min_out {
        let why = format!("проба исполнителя: получено {got} < minOut {}", c.min_out);
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    Ok((got, r.gas_used.to::<u64>()))
}
