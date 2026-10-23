//! Файл: venues/bsc_dex/quote/quoter.rs
//! Что делает: выход обмена по квотеру своей DEX через eth_call (точный вход) — для сверки локальной математики
//! и для измерения влияния крючка: V2 — Router.getAmountsOut; V3 — QuoterV2; Infinity CL/Bin — CLQuoter/BinQuoter;
//! Uniswap V4 — V4Quoter. Квотер исполняет настоящий обмен (с крючками) и откатывается — research/flow/bsc_v4.md, раздел 5.
//! Сверка: |локально − квотер| / квотер · 100 > dex.quoter_max_diff_pct → пул исключается до следующей сверки.
//! Кто вызывает: check/cycle.rs (сверка по циклу dex.quoter_check_s), check/hooks_probe.rs.
//! От чего зависит: node/rpc.rs, quote/quoter_calls.rs, pools/registry.rs, quote/pool_quote.rs.
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
use super::{pool_quote, quoter_calls};

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

/// Сверка пула: локальная математика против квотера на одной сумме.
/// Принимает: запись, токен входа, сумму. Возвращает: Ok(()) — в пределах допуска; Err(причина) — нет.
/// Ошибки: настройки, узел.
pub async fn compare(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
) -> GatewayResult<Result<(), String>> {
    let limit = ctx.dec("dex.quoter_max_diff_pct")?;
    let local = pool_quote::swap(ctx, e, token_in, true, amount)?.amount_out;
    let remote = out(ctx, e, token_in, amount).await?;
    let diff = diff_pct(local, remote);
    Ok(match diff {
        Some(d) if d <= limit => Ok(()),
        Some(d) => Err(format!(
            "расчёт {local} против квотера {remote}: расхождение {d}% > {limit}% (dex.quoter_max_diff_pct)"
        )),
        None => Err(format!(
            "расчёт {local} против квотера {remote}: сравнить нельзя"
        )),
    })
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
