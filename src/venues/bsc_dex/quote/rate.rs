//! Файл: venues/bsc_dex/quote/rate.rs
//! Что делает: пересчёт сумм между USDT и встречным токеном пула (промежуточной монетой, решение 52) по живому
//! состоянию лучшего достоверного пула «промежуточная — USDT» реестра — локальной математикой пула (та же, что
//! у котировки). Нужен там, где сумма задана в USDT, а пул — с промежуточной монетой: сумма пробы
//! (check.probe_usdt) и ликвидность пула против dex.min_pool_liquidity_usd.
//! Кто вызывает: check/taxes.rs, check/hooks_probe.rs, check/quoter_cycle.rs, quote/select.rs.
//! От чего зависит: pools/registry.rs, quote/pool_quote.rs, ctx.rs.
//! Настройки: через pool_quote — dex.v2_fee_pct [вид пула V2].

use alloy_primitives::{Address, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::registry::Registry;
use super::pool_quote;

/// Сколько единиц встречного токена `quote` дают `amount` единиц USDT (точный вход, лучший пул quote–USDT).
/// quote = USDT — та же сумма. Ошибки: NoRoute — нет достоверного пула «quote — USDT», расчёт невозможен.
pub fn from_usdt(
    ctx: &Ctx,
    reg: &Registry,
    usdt: Address,
    quote: Address,
    amount: U256,
) -> GatewayResult<U256> {
    best(ctx, reg, usdt, quote, usdt, amount)
}

/// Сколько единиц USDT дают `amount` единиц встречного токена `quote` (точный вход, лучший пул quote–USDT).
/// quote = USDT — та же сумма. Ошибки: NoRoute — нет достоверного пула «quote — USDT», расчёт невозможен.
pub fn to_usdt(
    ctx: &Ctx,
    reg: &Registry,
    usdt: Address,
    quote: Address,
    amount: U256,
) -> GatewayResult<U256> {
    best(ctx, reg, usdt, quote, quote, amount)
}

/// Наибольший выход по пулам «quote — USDT» на точный вход token_in. Ошибки: NoRoute.
fn best(
    ctx: &Ctx,
    reg: &Registry,
    usdt: Address,
    quote: Address,
    token_in: Address,
    amount: U256,
) -> GatewayResult<U256> {
    if quote == usdt {
        return Ok(amount);
    }
    reg.by_pair(quote, usdt)
        .iter()
        .filter(|e| e.valid)
        .filter_map(|e| pool_quote::swap(ctx, e, token_in, true, amount).ok())
        .filter(|s| s.complete)
        .map(|s| s.amount_out)
        .max()
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                format!("нет достоверного пула {quote} — USDT для пересчёта суммы"),
            )
        })
}
