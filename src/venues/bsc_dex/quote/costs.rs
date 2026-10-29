//! Файл: venues/bsc_dex/quote/costs.rs
//! Что делает: котировка в формате ядра из путей (один путь — прямой режим; несколько — доли умного маршрута):
//! средняя цена, исполнимое количество, маршрут (доли и пулы по шагам) и все затраты этой стороны по шагам маршрута
//! (quote/steps.rs: газ, комиссия пула, налог, крючок каждого обмена; решения 35, 52). Одинаковые затраты долей
//! складываются по виду, активу и шагу. Газ разрешений и чаевые добавляет quote/mod.rs.
//! Кто вызывает: quote/mod.rs, quote/smart_quote.rs.
//! От чего зависит: quote/path.rs, quote/steps.rs, core/types.
//! Настройки: через steps.rs — dex.v2_fee_pct [вид пула V2].
//!
//! Виды затрат, правило in_price и шаг маршрута — core/types/cost.rs (стык); газ = gasUsed пробного обмена × цена газа.

use alloy_primitives::U256;
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, Cost, DexRoute, Quote, RouteLeg, RouteMode, Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::path::Path;
use super::steps::{self, Names};

/// Сборка котировки. Принимает: монету, сторону, запрошенное количество, пути, режим маршрута, налоги, цену газа (wei),
/// тикеры. Ошибки: NoRoute (путей нет или монет 0), BadResponse (числа, тикер), NotConfigured (dex.v2_fee_pct).
#[allow(clippy::too_many_arguments)]
pub fn quote(
    ctx: &Ctx,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    paths: &[Path],
    mode: RouteMode,
    t: &TokenTaxes,
    gas_wei: U256,
    names: &Names<'_>,
) -> GatewayResult<Quote> {
    let coins: Decimal = paths.iter().map(|p| p.coins).sum();
    let usdt: Decimal = paths.iter().map(|p| p.usdt).sum();
    if coins.is_zero() {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            format!("{}: маршрут не даёт ни одной монеты", asset.ticker),
        ));
    }
    let input = |p: &Path| if side == Side::Buy { p.usdt } else { p.coins };
    let total_in = if side == Side::Buy { usdt } else { coins };
    let mut costs: Vec<Cost> = Vec::new();
    let mut route = DexRoute {
        mode,
        legs: Vec::new(),
        block: u64::MAX,
        gas_units: 0,
        // Прямой и умный маршруты исполнимы одной транзакцией; графовый — выставляет исполнимость quote/graph_route.rs.
        executable: true,
        exec_note: None,
    };
    for p in paths {
        for c in steps::path_costs(ctx, p, side, t, gas_wei, names)? {
            merge(&mut costs, c);
        }
        for h in &p.hops {
            route.block = route.block.min(h.entry.pool.block);
        }
        route.gas_units += p.gas_units();
        route.legs.push(RouteLeg {
            share: if total_in.is_zero() {
                Decimal::ZERO
            } else {
                input(p) / total_in
            },
            hops: p.hops.iter().map(|h| h.route_hop()).collect(),
            expected_out: p.expected_out(side),
        });
    }
    Ok(Quote {
        venue: ctx.venue(),
        asset: asset.clone(),
        side,
        qty,
        fillable_qty: coins,
        avg_price: usdt / coins,
        costs,
        route: Some(route),
        signal_route: None,
        at: Utc::now(),
    })
}

/// Добавляет затрату: такая же (вид, актив, в цене ли, шаг) уже есть — суммы складываются. Ошибок нет.
fn merge(all: &mut Vec<Cost>, c: Cost) {
    match all.iter_mut().find(|x| {
        x.kind == c.kind && x.asset == c.asset && x.in_price == c.in_price && x.step == c.step
    }) {
        Some(x) => x.amount += c.amount,
        None => all.push(c),
    }
}
