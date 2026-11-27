//! Подписанные открытые заявки Gladius. Один билет оплачивается собственным ERC20-инвентарём.
//! https://docs.rubicon.finance/guides/integration/filler
use super::{ctx::Ctx, node::rpc, parts::Parts, quote::white_state};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use alloy_primitives::{Address, B256, Bytes, U256};
use chrono::Utc;
mod abi;
pub(super) mod book;
mod model;
mod profile;
pub(super) mod routing;
pub(super) mod settlement;
mod simulate;
#[cfg(test)]
mod tests;
mod witness;

pub(super) const LABEL: &str = "Rubicon Gladius";
pub(super) fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("gladius.trading").unwrap_or(false)
}
pub(super) fn timely(ctx: &Ctx, deadline: u64) -> bool {
    ctx.int("gladius.min_validity_ms").is_ok_and(|ms| {
        ms > 0
            && deadline.checked_mul(1000).is_some_and(|d| {
                d.saturating_sub(Utc::now().timestamp_millis().max(0) as u64) >= ms
            })
    })
}
fn no(ctx: &Ctx, reason: &str) -> GatewayError {
    ctx.err(ErrorCode::NoRoute, reason)
}
