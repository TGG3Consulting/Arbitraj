//! Файл: venues/bsc_dex/quote/quoter.rs
//! Что делает: выход обмена по квотеру своей DEX через eth_call (точный вход) — для сверки локальной математики
//! и для измерения влияния крючка: V2 — Router.getAmountsOut; V3 — QuoterV2; Infinity CL/Bin — CLQuoter/BinQuoter;
//! Uniswap V4 — V4Quoter. Квотер исполняет настоящий обмен (с крючками) и откатывается — research/flow/bsc_v4.md, раздел 5.
//! Сверка: |локально − квотер| / квотер · 100 > dex.quoter_max_diff_pct → пул исключается до следующей сверки.
//! У пула с крючком B/C квотер исполняет крючок, а математика — нет: локальный выход берётся за вычетом измеренного
//! влияния крючка (check/hooks_probe.rs) — так сверяются и математика пула, и то, что крючок ведёт себя как при пробе.
//! Кто вызывает: check/quoter_cycle.rs (цикл dex.quoter_check_s), check/hooks_probe.rs, exec/precheck.rs (перед сделкой),
//! quote/mixed_quoter.rs (less_hook, diff_pct).
//! От чего зависит: node/rpc.rs, quote/quoter_calls.rs, pools/registry.rs, quote/pool_quote.rs, quote/emptiness.rs.
//! Настройки: dex.router (V2), dex.quoter (V3, V4) [вид пула], dex.quoter_max_diff_pct [bsc_dex].
//!
//! Источники: шапка node/abi_dex.rs; «When to use quoter: сверка локального расчёта, финальная проверка» —
//! research/flow/bsc_dex.md, раздел 3, bsc_v4.md, раздел 5.

use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::super::pools::registry::Entry;
use super::{emptiness, pool_quote, quoter_calls, quoter_revert};

/// Выход по квотеру на точный вход (eth_call). Ошибки: NotConfigured (адрес), узел (откат квотера — его текст),
/// BadResponse (ответ не по ABI).
pub async fn out(ctx: &Ctx, e: &Entry, token_in: Address, amount: U256) -> GatewayResult<U256> {
    let (to, data) = quoter_calls::request(ctx, e, token_in, amount)?;
    let raw: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{ "to": to, "input": data }, "latest"]),
    )
    .await?;
    quoter_calls::decode(e, &raw).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{}: ответ квотера не по ABI", e.family.title),
        )
    })
}

/// Сверка пула: локальная математика (за вычетом влияния крючка) против квотера на одной сумме.
/// Пустой пул и неполный локальный расчёт (сумма не прошла в прочитанной карте тиков) — квотер не вызывается: это не
/// расхождение, а честная причина исключения (quote/emptiness.rs). Откат квотера «не хватает ликвидности»
/// (quote/quoter_revert.rs) — та же причина, а не сбой.
/// Принимает: запись, токен входа, сумму, влияние крючка на эту сторону, % (0 — крючка B/C нет).
/// Возвращает: Ok(()) — в пределах допуска; Err(причина) — нет. Ошибки: настройки, узел.
pub async fn compare(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
    hook_pct: Decimal,
) -> GatewayResult<Result<(), String>> {
    if emptiness::empty(e) {
        return Ok(Err(emptiness::EMPTY.into()));
    }
    let local = pool_quote::swap(ctx, e, token_in, true, amount)?;
    if !local.complete {
        return Ok(Err(emptiness::short(emptiness::SHORT_LOCAL)));
    }
    let remote = match out(ctx, e, token_in, amount).await {
        Ok(r) => r,
        Err(err) => {
            return match quoter_revert::liquidity_short(&err) {
                Some(why) => Ok(Err(emptiness::short(why))),
                None => Err(err),
            };
        }
    };
    within(ctx, local.amount_out, remote, hook_pct)
}

/// Локальный выход (за вычетом крючка) против выхода квотера: в пределах dex.quoter_max_diff_pct?
/// Возвращает: Ok(()) или Err(причина с цифрами). Ошибки: NotConfigured (допуск).
pub fn within(
    ctx: &Ctx,
    local: U256,
    remote: U256,
    hook_pct: Decimal,
) -> GatewayResult<Result<(), String>> {
    let limit = ctx.dec("dex.quoter_max_diff_pct")?;
    let Some(local) = less_hook(local, hook_pct) else {
        return Ok(Err(format!("влияние крючка {hook_pct}% не считается")));
    };
    Ok(match diff_pct(local, remote) {
        Some(d) if d <= limit => Ok(()),
        Some(d) => Err(format!(
            "расчёт {local} против квотера {remote}: расхождение {d}% > {limit}% (dex.quoter_max_diff_pct)"
        )),
        None => Err(format!(
            "расчёт {local} против квотера {remote}: сравнить нельзя"
        )),
    })
}

/// Выход за вычетом влияния крючка: out · (1e6 − pct·1e4) / 1e6 (миллионные доли, как пипсы комиссий).
/// Отрицательное влияние (крючок даёт больше) не прибавляется. None — pct больше 100% или не число.
pub fn less_hook(out: U256, pct: Decimal) -> Option<U256> {
    let million = U256::from(1_000_000u32);
    let ppm = (pct.max(Decimal::ZERO) * Decimal::from(10_000u32)).round();
    let ppm = U256::from(u64::try_from(ppm).ok()?);
    if ppm > million {
        return None;
    }
    Some(out.saturating_mul(million - ppm) / million)
}

/// |a − b| / b · 100. None — b = 0 или числа не помещаются в Decimal.
pub fn diff_pct(a: U256, b: U256) -> Option<Decimal> {
    let (a, b) = (to_dec(a)?, to_dec(b)?);
    if b.is_zero() {
        return None;
    }
    Some((a - b).abs() / b * Decimal::ONE_HUNDRED)
}

/// U256 → Decimal без масштаба. None — больше 28 знаков.
fn to_dec(v: U256) -> Option<Decimal> {
    super::super::num::to_decimal(v, 0)
}
