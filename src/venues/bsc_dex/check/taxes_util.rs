//! Файл: venues/bsc_dex/check/taxes_util.rs
//! Что делает: части проверки налогов: налоги в процентах по замеру пробы и выбор источника встречного токена для пробы.
//! Кто вызывает: check/taxes.rs.
//! От чего зависит: check/probe.rs (Measured), pools/registry.rs, node/call.rs, num.rs.
//! Настроек нет.
//!
//! Налог, % = (ожидалось − пришло) / ожидалось · 100 — research/flow/bsc_dex.md, раздел 6.
//! balanceOf — EIP-20: https://eips.ethereum.org/EIPS/eip-20#balanceof

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolRef, TokenTaxes};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::num;
use super::super::pools::registry::Registry;
use super::probe::Measured;

/// Налоги в процентах. None — делить не на что (ожидалось 0).
pub fn percent(m: &Measured, decimals: u8) -> Option<TokenTaxes> {
    let pct = |sent: U256, got: U256| -> Option<Decimal> {
        let s = num::to_decimal(sent, decimals)?;
        let g = num::to_decimal(got.min(sent), decimals)?;
        if s.is_zero() {
            None
        } else {
            Some((s - g) / s * Decimal::ONE_HUNDRED)
        }
    };
    Some(TokenTaxes {
        buy_pct: pct(m.buy_expected, m.buy_got)?,
        sell_pct: pct(m.sell_sent, m.sell_got)?,
        transfer_pct: pct(m.transfer_sent, m.transfer_got)?,
    })
}

/// Источник встречного токена (USDT или промежуточной монеты) для пробы: пул V2/V3 реестра (не проверяемый)
/// с наибольшим балансом этого токена ≥ суммы пробы. Ошибки: узел. None — такого пула нет.
pub async fn quote_source(
    ctx: &Ctx,
    reg: &Registry,
    usdt: Address,
    exclude: Option<Address>,
    need: U256,
) -> GatewayResult<Option<Address>> {
    let list: Vec<Address> = reg
        .refs()
        .into_iter()
        .filter_map(|r| match r {
            PoolRef::Contract(a) if Some(a) != exclude => Some(a),
            _ => None,
        })
        .collect();
    let calls: Vec<_> = list
        .iter()
        .map(|a| IERC20::balanceOfCall { owner: *a })
        .collect();
    let balances = call::view_many(ctx, usdt, &calls, "latest").await?;
    Ok(list
        .into_iter()
        .zip(balances)
        .filter(|(_, b)| *b >= need)
        .max_by_key(|(_, b)| *b)
        .map(|(a, _)| a))
}
