//! Личная котировка 0x на Ethereum. В книге хранятся только проверенные предложения с согласием владельца.
pub(super) mod profile;
pub(super) mod receipt;
pub(super) mod routing;
mod trade;
pub(super) use profile::{module, verify};
pub(super) use routing::Book;
pub(super) use trade::trade;
pub(super) const LABEL: &str = "0x";
use super::ctx::Ctx;
pub(super) fn enabled(ctx: &Ctx) -> bool {
    ctx.chain_id() == Some(1) && ctx.flag("zeroex.trading").unwrap_or(false)
}
pub(super) fn current(until: u64) -> bool {
    chrono::Utc::now().timestamp().unsigned_abs() < until
}
#[cfg(test)]
mod tests;
