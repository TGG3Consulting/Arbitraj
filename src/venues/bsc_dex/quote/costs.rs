//! Файл: venues/bsc_dex/quote/costs.rs
//! Что делает: котировка в формате ядра из лучшего пула — средняя цена, исполнимое количество, маршрут и затраты
//! этой стороны (решение 35): комиссия пула, налог покупки/продажи, влияние крючка (всё это уже в цене,
//! in_price = true — для прозрачности) и газ обмена в BNB (вычитается отдельно).
//! Газ разрешений (ApproveGas) добавляет quote/mod.rs, если разрешений ещё нет; чаевые канала — фактом в
//! ChainTx.tip_bnb (только при отправке в один канал бандлом).
//! Кто вызывает: quote/mod.rs.
//! От чего зависит: quote/direct.rs (Candidate), quote/pool_quote.rs (комиссия V4), num.rs, core/types.
//! Настройки: dex.v2_fee_pct [вид пула V2].
//!
//! Виды затрат и правило in_price — core/types/cost.rs и trade.rs (стык); газ = gasUsed пробного обмена × цена газа.

use alloy_primitives::U256;
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{
    Asset, Cost, CostKind, DexRoute, PoolKind, PoolState, Quote, RouteHop, RouteLeg, RouteMode,
    Side, TokenTaxes,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::wallet::balances::BNB;
use super::direct::Candidate;
use super::pool_quote::swap_fee;

/// Сборка котировки. Принимает: монету, сторону, запрошенное количество, лучший пул, налоги, цену газа (wei), USDT-тикер.
/// Ошибки: BadResponse (числа), NotConfigured (dex.v2_fee_pct).
#[allow(clippy::too_many_arguments)]
pub fn quote(
    ctx: &Ctx,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    c: &Candidate,
    t: &TokenTaxes,
    gas_wei: U256,
    usdt: &str,
) -> GatewayResult<Quote> {
    let bad = |m: &str| ctx.err(ErrorCode::BadResponse, m.to_string());
    if c.coins.is_zero() {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            format!("{}: пул не даёт ни одной монеты", asset.ticker),
        ));
    }
    let e = &c.entry;
    let gas_bnb = num::to_decimal(
        gas_wei.saturating_mul(U256::from(c.usable.gas)),
        num::BNB_DECIMALS,
    )
    .ok_or_else(|| bad("газ не помещается в Decimal"))?;
    let fee_pct = fee_pct(ctx, &c.entry, side)?;
    let pct = |v: Decimal, p: Decimal| v * p / Decimal::ONE_HUNDRED;
    let mut costs = vec![Cost::new(CostKind::Gas, gas_bnb, BNB, false)];
    match side {
        Side::Buy => {
            costs.push(Cost::new(
                CostKind::PoolFee,
                pct(c.usdt, fee_pct),
                usdt,
                true,
            ));
            costs.push(Cost::new(
                CostKind::BuyTax,
                pct(c.coins, t.buy_pct),
                &asset.ticker,
                true,
            ));
            costs.push(Cost::new(
                CostKind::HookImpact,
                pct(c.coins, c.usable.hook_pct),
                &asset.ticker,
                true,
            ));
        }
        Side::Sell => {
            costs.push(Cost::new(
                CostKind::PoolFee,
                pct(c.coins, fee_pct),
                &asset.ticker,
                true,
            ));
            costs.push(Cost::new(
                CostKind::SellTax,
                pct(c.coins, t.sell_pct),
                &asset.ticker,
                true,
            ));
            costs.push(Cost::new(
                CostKind::HookImpact,
                pct(c.usdt, c.usable.hook_pct),
                usdt,
                true,
            ));
        }
    }
    let expected_out = match side {
        Side::Buy => c.coins,
        Side::Sell => c.usdt,
    };
    let route = DexRoute {
        mode: RouteMode::Direct,
        legs: vec![RouteLeg {
            share: Decimal::ONE,
            hops: vec![RouteHop {
                family: e.family.id.to_string(),
                kind: e.pool.kind,
                pool: e.pool.reference.clone(),
            }],
            expected_out,
        }],
        block: e.pool.block,
        gas_units: c.usable.gas,
    };
    Ok(Quote {
        venue: ctx.venue(),
        asset: asset.clone(),
        side,
        qty,
        fillable_qty: c.coins,
        avg_price: c.usdt / c.coins,
        costs,
        route: Some(route),
        at: Utc::now(),
    })
}

/// Полная комиссия пула в процентах для направления стороны. Ошибки: NotConfigured (V2), BadResponse.
fn fee_pct(
    ctx: &Ctx,
    e: &super::super::pools::registry::Entry,
    side: Side,
) -> GatewayResult<Decimal> {
    let pips = |p: u32| Decimal::from(p) / Decimal::from(10_000u32);
    // Покупка: отдаём USDT; zeroForOne — если USDT — token0.
    let zero_for_one = (side == Side::Buy) == (e.usdt == e.pool.token0);
    match (&e.pool.state, e.pool.kind) {
        (_, PoolKind::V2) => ctx.dex_dec("dex.v2_fee_pct", e.family.id),
        (_, PoolKind::V3) => e
            .pool
            .fee_pips
            .map(pips)
            .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "у пула V3 нет fee")),
        (
            PoolState::Cl {
                protocol_fee,
                lp_fee,
                ..
            }
            | PoolState::Bin {
                protocol_fee,
                lp_fee,
                ..
            },
            _,
        ) => Ok(pips(swap_fee(*protocol_fee, *lp_fee, zero_for_one))),
        _ => Err(ctx.err(ErrorCode::BadResponse, "состояние пула не того вида")),
    }
}
