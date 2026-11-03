//! Файл: venues/bsc_dex/quote/graph_split.rs
//! Что делает: жадный сплит объёма между путями графа (решения 38, 80; вынесен из quote/graph_route.rs). Объём режется
//! на шаги smart.split_step_pct, каждый шаг отдаётся пути с лучшей предельной отдачей (покупка — меньше USDT за шаг,
//! продажа — больше), не больше smart.max_splits путей.
//! Решение 122, часть R1 (учёт газа, как `gasInclude` у KyberSwap): при dex.route_gas_aware = да отдача шага — ЧИСТАЯ:
//! открытие нового пути стоит его газа в USDT (quote/route_gas.rs), увеличение уже открытой доли газа не добавляет
//! (газ обмена от суммы почти не зависит — он измерен пробой на пул). Покупка: цена шага = USDT шага + газ нового пути;
//! продажа: отдача шага = USDT шага − газ нового пути. Поэтому выбор между путями разной длины — тоже по чистому
//! выходу, а новая доля добавляется, только если её чистая отдача лучше увеличения уже открытой. Продажа: новая доля
//! при уже открытой с чистой отдачей ≤ 0 не открывается вовсе (газ съедает прирост) — остаток объёма тогда не
//! исполняется (маршрут не делает обмен себе в убыток). Не задано / «нет» — сплит без газа, как до решения 122.
//! Кто вызывает: quote/graph_route.rs.
//! От чего зависит: quote/route_calc.rs (Leg, eval), quote/path.rs (Path).
//! Настройки: smart.split_step_pct, smart.max_splits [bsc_dex]; учёт газа включает вызывающий (dex.route_gas_aware).

use rust_decimal::Decimal;

use crate::core::types::Side;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::path::Path;
use super::route_calc;

/// Путь-кандидат сплита: ноги в порядке исполнения, сколько монет ему уже отдано, его расчёт и газ пути в USDT
/// (0 — учёт газа выключен).
pub struct Cand {
    pub legs: Vec<route_calc::Leg>,
    pub qty: Decimal,
    pub got: Option<Path>,
    pub gas_usdt: Decimal,
}

impl Cand {
    /// Кандидат без отданного объёма. Ошибок нет.
    pub fn new(legs: Vec<route_calc::Leg>, gas_usdt: Decimal) -> Self {
        Self {
            legs,
            qty: Decimal::ZERO,
            got: None,
            gas_usdt,
        }
    }
}

/// Чистая отдача шага: прирост USDT шага (покупка — отдадим больше, продажа — получим больше) с газом нового пути.
/// Покупка — меньше лучше, продажа — больше лучше. Ошибок нет.
fn score(side: Side, delta: Decimal, opens: bool, gas: Decimal) -> Decimal {
    let g = if opens { gas } else { Decimal::ZERO };
    match side {
        Side::Buy => delta + g,
        Side::Sell => delta - g,
    }
}

/// Сплит. Принимает: окружение, кандидатов, сторону, количество монет, долю keep налога монеты, учитывать ли газ.
/// Возвращает: доли (пути). Ошибки: NotConfigured (шаг/число долей), NoRoute (ни один путь не принял объём).
pub fn split(
    ctx: &Ctx,
    cands: &mut [Cand],
    side: Side,
    qty: Decimal,
    tax_keep: Decimal,
    gas_aware: bool,
) -> GatewayResult<Vec<Path>> {
    let max_splits = usize::try_from(ctx.int("smart.max_splits")?).unwrap_or(usize::MAX);
    let step_pct = ctx.dec("smart.split_step_pct")?;
    if step_pct <= Decimal::ZERO || step_pct > Decimal::ONE_HUNDRED || max_splits == 0 {
        return Err(ctx.not_set(
            "smart.split_step_pct",
            "smart.split_step_pct должен быть в (0; 100], smart.max_splits — больше нуля",
        ));
    }
    let unit = qty * step_pct / Decimal::ONE_HUNDRED;
    let mut left = qty;
    while left > Decimal::ZERO {
        let step = unit.min(left);
        let used = cands.iter().filter(|c| c.got.is_some()).count();
        let mut pick: Option<(usize, Path, Decimal)> = None;
        for (i, c) in cands.iter().enumerate() {
            let opens = c.got.is_none();
            if opens && used >= max_splits {
                continue;
            }
            let Ok(p) = route_calc::eval(ctx, &c.legs, side, c.qty + step, tax_keep) else {
                continue;
            };
            let before = c.got.as_ref().map_or(Decimal::ZERO, |g| g.usdt);
            let s = score(side, p.usdt - before, opens, c.gas_usdt);
            // R1: продажа — новая доля рядом с уже открытой не открывается, если газ съедает её прирост.
            if gas_aware && opens && used > 0 && side == Side::Sell && s <= Decimal::ZERO {
                continue;
            }
            let better = pick.as_ref().is_none_or(|(_, _, d)| match side {
                Side::Buy => s < *d,
                Side::Sell => s > *d,
            });
            if better {
                pick = Some((i, p, s));
            }
        }
        let Some((i, p, _)) = pick else { break };
        cands[i].qty += step;
        cands[i].got = Some(p);
        left -= step;
    }
    let legs: Vec<Path> = cands.iter_mut().filter_map(|c| c.got.take()).collect();
    if legs.is_empty() {
        return Err(ctx.err(ErrorCode::NoRoute, "граф: ни один путь не принял объём"));
    }
    Ok(legs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Газ считается только при открытии пути и в нужную сторону. Ошибок нет.
    #[test]
    fn score_charges_gas_on_open_only() {
        let (d, g) = (Decimal::from(10), Decimal::from(3));
        assert_eq!(score(Side::Buy, d, true, g), Decimal::from(13));
        assert_eq!(score(Side::Buy, d, false, g), d);
        assert_eq!(score(Side::Sell, d, true, g), Decimal::from(7));
        assert_eq!(score(Side::Sell, d, false, g), d);
    }
}
