//! Файл: venues/bsc_dex/exec/cycle_bundle_trade.rs
//! Что делает: сделка цикла DEX ↔ DEX атомарным бандлом свопов через Universal Router (решения 84, 86, 124) — форма
//! (a), когда исполнителя нет (exec/cycle_gate.rs: все шаги через UR, все каналы бандловые). Сборка — cycle_bundle
//! (участки по роутерам, межроутерный переход по гарантированному минимуму, последний хоп несёт границу ядра minOut
//! через BALANCE_CHECK). Перед отправкой — СВОЯ симуляция всего бандла (eth_simulateV1) с замером баланса базовой
//! монеты кошелька до первой и после последней транзакции: настоящий выход цикла, газ всех транзакций (свопы и
//! разрешения Permit2) → чистая прибыль ≥ dex_dex.min_profit_usd (симуляция отправки bundle_send ставит базу проверки
//! баланса 0 и границу не проверяет — поэтому замер здесь). Затем — bundle_send::send (симуляция, «кто меняет»,
//! подпись, eth_sendBundle, ожидание блока, settle очереди) и факт по объединённым логам (bundle_trade::bundle_result).
//! Бандл не попал в блок — истекает по maxBlockNumber, газа нет, nonce свободны (решение 84). Повторов нет.
//! Исход бандла неизвестен — «исход неизвестен» с «чем найти» цикла (exec/cycle_ref.rs + BundleRef, решение 233,
//! дополнение, п. 4): ядро выясняет исход само (resolve_cycle.rs → resolve_bundle.rs).
//! Кто вызывает: exec/cycle_trade.rs.
//! От чего зависит: exec/{cycle_bundle, cycle_steps, cycle_ref, bundle_ref, bundle_sim, bundle_send, bundle_trade, bundle_notify}.rs,
//! check/{simulate, sim_base, probe_calls}.rs, wallet/nonce.rs.
//! Настройки: send.gas_limit_margin_pct [bsc_dex]; через вызываемые — send.*, channel.*, dex.*, dex_dex.min_profit_usd.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{CycleTradeRequest, CycleTradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::probe_calls as pc;
use super::super::check::sim_base;
use super::super::check::simulate::{self, Base};
use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::registry::Entry;
use super::super::quote::Quoting;
use super::super::quote::cycle_cost::Rates;
use super::super::quote::route_calc::Leg;
use super::super::wallet::nonce::Nonces;
use super::bundle::Bundle;
use super::bundle_ref::BundleRef;
use super::bundle_sim::{build_sim, collect};
use super::cycle_steps::{self, net_check, units};
use super::txs::Env;
use super::{bundle_notify, bundle_send, bundle_trade, cycle_bundle, cycle_ref};

/// Сделка бандлом. Принимает: котировку, окружение, очередь кошелька, запрос, свежие ноги, курсы. Ошибки: NoRoute
/// (хоп не собирается, симуляция откатилась), QuoteMismatch (ниже границы / газ съел прибыль), настройки, каналы, узел,
/// TxNotIncluded / SwapReverted / OutcomeUnknown (с фактом бандла).
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: &CycleTradeRequest,
    legs: Vec<Leg>,
    r: Rates,
) -> GatewayResult<CycleTradeResult> {
    let ctx = &q.ctx;
    let dec = legs[0].dec_in;
    let base = req.cycle.base;
    let floor = units(ctx, req.min_out, dec)?;
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let first_nonce = slot.nonce(ctx).await?;
    // Решение 127: срок бандла и замер — от одной головы; симулированный блок на 1 с позже (check/sim_base.rs).
    let (at, now) = sim_base::head(ctx, &q.heads).await?;
    let at4 = (wallet, first_nonce, now, false);
    let a = cycle_bundle::build_floor(ctx, &legs, (req.input, floor), at4).await?;
    let (out, gas) = measure(ctx, &a.bundle, wallet, base, at, (req.input, dec)).await?;
    net_check(ctx, (out, req.input), (gas, cycle_steps::wei(q).await?), &r).await?;
    let b = &a.bundle;
    bundle_notify::assembled(&env.stats, ctx, b.swaps, 1, b.txs.len(), first_nonce);
    let entries: Vec<&Entry> = legs.iter().map(|l| &l.entry).collect();
    let sent = bundle_send::send(env, &slot, b, &entries).await?;
    drop(slot);
    let tr = cycle_steps::trade_request(req);
    // Решение 233 (дополнение, п. 4): исход бандла неизвестен — «чем найти» цикла (контекст свопа «базовая → базовая»,
    // хеши всех транзакций, первый nonce, maxBlockNumber, курсы сделки), позднее выяснение — resolve_cycle.rs.
    let mut sc = cycle_ref::ctx(wallet, base, dec, &r);
    sc.bundle = Some(BundleRef::of(&sent, first_nonce));
    let t = bundle_trade::bundle_result(
        ctx,
        tr,
        &sent,
        wallet,
        (base, base),
        (dec, dec),
        (first_nonce, &[]),
    )
    .map_err(|e| sc.attach(e))?;
    Ok(cycle_steps::finish(t, &r))
}

/// Своя симуляция бандла: баланс базовой у кошелька до первой и после последней транзакции, газ всех транзакций.
/// Принимает: бандл, кошелёк, базовую, базовый блок (голова + 1 с, решение 127), (вход, decimals базовой). Возвращает: выход цикла (вход + прирост базовой)
/// и газ. Ошибки: NoRoute (откат, итог меньше границы — выход ниже входа), BadResponse, узел.
pub async fn measure(
    ctx: &Ctx,
    b: &Bundle,
    wallet: Address,
    base: Address,
    at: Base,
    (input, dec): (Decimal, u8),
) -> GatewayResult<(Decimal, u64)> {
    let plan = build_sim(b, wallet);
    let mut calls = vec![pc::balance(wallet, base, wallet)];
    calls.extend(plan.calls.iter().cloned());
    calls.push(pc::balance(wallet, base, wallet));
    let values: Vec<(usize, U256)> = plan.values.iter().map(|(i, v)| (i + 1, *v)).collect();
    let res = simulate::run_valued(ctx, &calls, &values, at).await?;
    let margin = ctx.dec("send.gas_limit_margin_pct")?;
    collect(ctx, &res[1..res.len() - 1], &plan, margin)?;
    let gas: u64 = plan
        .main
        .iter()
        .map(|&i| res[i + 1].gas_used.to::<u64>())
        .sum();
    let read = |i: usize| pc::u256(&res[i]);
    let (Some(before), Some(after)) = (read(0), read(res.len() - 1)) else {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "баланс базовой в симуляции не прочитан",
        ));
    };
    let input_u = units(ctx, input, dec)?;
    let back = after.saturating_add(input_u);
    if back < before {
        let why =
            format!("симуляция бандла: вернулось меньше вложенного ({after} после, {before} до)");
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    let out = num::to_decimal(back - before, dec)
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "выход симуляции не в Decimal"))?;
    Ok((out, gas))
}
