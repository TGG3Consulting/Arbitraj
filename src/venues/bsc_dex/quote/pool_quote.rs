//! Файл: venues/bsc_dex/quote/pool_quote.rs
//! Что делает: обмен в одном пуле по его локальному состоянию — выбор математики по виду пула (V2, V3, CL, Bin),
//! полная комиссия (V2 — настройка, V3 — fee пула, V4 — протокольная + LP по направлению), точный вход/выход.
//! Кто вызывает: quote/direct.rs, quote/liquidity.rs, quote/quoter.rs (сверка), check/hooks_probe.rs.
//! От чего зависит: math/*, pools/registry.rs, ctx.rs.
//! Настройки: dex.v2_fee_pct [DEX V2] — если у пары нет своей комиссии (dex.v2_fee_source = pool).
//!
//! Комиссия V4: протокольная по направлению — нижние 12 бит (zeroForOne) или верхние 12 (oneForZero);
//! swapFee = p + lp − p·lp/1e6 (p = 0 → lp) — ProtocolFeeLibrary.calculateSwapFee:
//! https://github.com/Uniswap/v4-core/blob/bea7f6d1c11cd5dc4f0d5151ffa37a1c239feee6/src/libraries/ProtocolFeeLibrary.sol#L17-L48 ,
//! infinity-core src/libraries/ProtocolFeeLibrary.sol#L51-L59; применение — Pool.sol#L300-L307, BinPool.sol#L124-L129.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::swap_cl::{self, ClState, Swapped, Version};
use super::super::math::{bin_swap, v2};
use super::super::pools::registry::Entry;

/// PIPS_DENOMINATOR = 1e6.
const PIPS: u64 = 1_000_000;

/// Обмен в пуле. Принимает: запись пула, какой токен отдаём, точный вход или выход, сумму в минимальных единицах.
/// Возвращает: Swapped. Ошибки: NotConfigured (dex.v2_fee_pct), NoRoute (нет карты/расчёт невозможен — как откат).
pub fn swap(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    exact_in: bool,
    amount: U256,
) -> GatewayResult<Swapped> {
    let zero_for_one = token_in == e.pool.token0;
    let why = |w: &str| {
        ctx.err(
            ErrorCode::NoRoute,
            format!(
                "{} {}: расчёт обмена невозможен: {w}",
                e.family.title, e.ticker
            ),
        )
    };
    let none = || {
        ctx.err(
            ErrorCode::NoRoute,
            format!(
                "{} {}: расчёт обмена невозможен (как откат контракта)",
                e.family.title, e.ticker
            ),
        )
    };
    match (&e.pool.state, e.pool.kind) {
        (PoolState::V2 { reserve0, reserve1 }, PoolKind::V2) => {
            let fee = v2::fee_ratio(v2_fee_pct(ctx, e)?)
                .ok_or_else(|| ctx.not_set("dex.v2_fee_pct", "комиссия пары V2 вне 0–100%"))?;
            let (r_in, r_out) = if zero_for_one {
                (*reserve0, *reserve1)
            } else {
                (*reserve1, *reserve0)
            };
            let r = if exact_in {
                v2::amount_out(amount, r_in, r_out, fee).map(|o| Swapped {
                    amount_in: amount,
                    amount_out: o,
                    complete: true,
                })
            } else {
                v2::amount_in(amount, r_in, r_out, fee).map(|i| Swapped {
                    amount_in: i,
                    amount_out: amount,
                    complete: true,
                })
            };
            r.ok_or_else(none)
        }
        (
            PoolState::V3 {
                sqrt_price_x96,
                liquidity,
                tick,
            },
            PoolKind::V3,
        ) => {
            let fee = e.pool.fee_pips.ok_or_else(none)?;
            let ticks = e.ticks.as_ref().ok_or_else(none)?;
            let s = ClState {
                sqrt_price: *sqrt_price_x96,
                tick: *tick,
                liquidity: *liquidity,
                fee,
                ticks,
                version: Version::V3,
            };
            swap_cl::swap(&s, zero_for_one, exact_in, amount).map_err(why)
        }
        (
            PoolState::Cl {
                sqrt_price_x96,
                tick,
                liquidity,
                protocol_fee,
                lp_fee,
            },
            PoolKind::Cl,
        ) => {
            let ticks = e.ticks.as_ref().ok_or_else(none)?;
            let fee = swap_fee(*protocol_fee, *lp_fee, zero_for_one);
            let s = ClState {
                sqrt_price: *sqrt_price_x96,
                tick: *tick,
                liquidity: *liquidity,
                fee,
                ticks,
                version: Version::V4,
            };
            swap_cl::swap(&s, zero_for_one, exact_in, amount).map_err(why)
        }
        (
            PoolState::Bin {
                active_id,
                protocol_fee,
                lp_fee,
            },
            PoolKind::Bin,
        ) => {
            let bins = e.bins.as_ref().ok_or_else(none)?;
            let fee = swap_fee(*protocol_fee, *lp_fee, zero_for_one);
            let amount: u128 = amount.try_into().map_err(|_| none())?;
            // Bin: X = currency0; swapForY = отдаём X (токен0).
            bin_swap::swap(bins, *active_id, fee, zero_for_one, exact_in, amount).ok_or_else(none)
        }
        _ => Err(none()),
    }
}

/// Полная комиссия V4 по направлению (пипсы). Ошибок нет.
pub fn swap_fee(protocol_fee: u32, lp_fee: u32, zero_for_one: bool) -> u32 {
    let p = u64::from(if zero_for_one {
        protocol_fee & 0xfff
    } else {
        protocol_fee >> 12
    });
    let lp = u64::from(lp_fee & 0xff_ffff);
    if p == 0 {
        return lp as u32;
    }
    (p + lp - p * lp / PIPS) as u32
}

/// Комиссия пары V2 в процентах: своя у пары (fee_pips — прочитана с пары при проверке, dex.v2_fee_source = pool)
/// или общая для DEX (dex.v2_fee_pct). Ошибки: NotConfigured (dex.v2_fee_pct).
pub fn v2_fee_pct(ctx: &Ctx, e: &Entry) -> GatewayResult<Decimal> {
    match e.pool.fee_pips {
        Some(p) => Ok(Decimal::from(p) / Decimal::from(10_000u32)),
        None => ctx.dex_dec("dex.v2_fee_pct", e.family.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Как ProtocolFeeLibrary.calculateSwapFee: 0,1% протокольной и 0,3% LP → 0,3997%.
    #[test]
    fn fee_composition() {
        assert_eq!(swap_fee(1000, 3000, true), 1000 + 3000 - 3);
        assert_eq!(swap_fee(1000 << 12, 3000, true), 3000);
        assert_eq!(swap_fee(1000 << 12, 3000, false), 3997);
    }
}
