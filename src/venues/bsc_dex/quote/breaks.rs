//! Файл: venues/bsc_dex/quote/breaks.rs
//! Что делает: решение 344 (задача t159, ревью t159 Б4, Н4) — изломы кривых пулов МАРШРУТА котировки по объёму монеты
//! всей стороны для подбора объёма ядра (контракт quote_breaks, core/size_breaks.rs). Пулы — только шаг монеты каждой
//! доли маршрута (продажа — первый хоп доли, монета на входе; покупка — последний, монета на выходе): изломы пулов
//! «промежуточная — USDT» в монету не переводятся (их место на кривой монеты зависит от первого шага) — не отдаются.
//! Перевод объёма на пуле x (quote/breaks_pool.rs) в объём монеты стороны q:
//! - продажа: на пул доли идёт q · доля · keep_налога (налог монеты списывается на входе) ⇒ q = x / (доля · keep);
//! - покупка: монета доли = выход пула · keep_крючка · keep_налога (quote/route_calc.rs), доля выхода — ожидаемый выход
//!   доли / сумма ожидаемых (RouteLeg.expected_out) ⇒ q = x · keep / доля.
//!
//! Доли сплита меняются с объёмом (жадный сплит), поэтому перевод — по долям маршрута лучшей точки; это точки
//! подбора, не данные решения. Налог и крючок — из допуска пула (select.rs usable, pool_tax.rs step_pct) и налогов
//! монеты из проверки (direct.rs taxes); не известны — keep = 1.
//! Только память реестра: ни одного запроса к узлу, кроме decimals монеты и USDT (кэш tokens.rs).
//! Решение «335 (лесенка влияния)», ревью 3: у пула с крючком B/C — ещё ступени лесенки влияния (quote/breaks_hook.rs).
//! Кто вызывает: venue.rs (контракт quote_breaks).
//! От чего зависит: quote/breaks_pool.rs, quote/select.rs, quote/pool_tax.rs, quote/route_calc.rs (keep),
//! quote/direct.rs (taxes), pools/registry.rs, tokens.rs, num.rs.
//! Настройки: нет (предел изломов — от ядра, size.pool_breaks_max).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Asset, DexRoute, Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::num;
use super::super::pools::registry::Entry;
use super::Quoting;
use super::route_calc::keep;

/// Что делает: изломы пулов маршрута (см. шапку). Принимает: окружение котировки, монету, сторону, маршрут,
/// наибольший объём (монет стороны), сколько изломов на пул наибольшим числом. Возвращает: объёмы монеты стороны по
/// возрастанию без повторов. Ошибки: NoRoute — нет адреса монеты; NotConfigured — тикер USDT; узел — decimals.
pub async fn breaks(
    q: &Quoting,
    asset: &Asset,
    side: Side,
    route: &DexRoute,
    max_qty: Decimal,
    limit: u32,
) -> GatewayResult<Vec<Decimal>> {
    let ctx = &q.ctx;
    let coin = asset.bsc_contract.ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            format!("{}: нет адреса контракта в BSC", asset.ticker),
        )
    })?;
    let dec = q.tokens.decimals(ctx, coin).await?;
    let usdt = q.tokens.usdt(ctx)?;
    let usdt_dec = q.tokens.decimals(ctx, usdt).await?;
    let taxes = super::direct::taxes(q, asset).ok();
    let sell = side == Side::Sell;
    let total_out: Decimal = route.legs.iter().map(|l| l.expected_out).sum();
    let mut out: Vec<Decimal> = Vec::new();
    for leg in &route.legs {
        let hop = if sell {
            leg.hops.first()
        } else {
            leg.hops.last()
        };
        let Some(e) = hop.and_then(|h| q.reg.get(&h.pool)) else {
            continue;
        };
        // Доля стороны через этот пул: продажа — доля входа; покупка — доля выхода монеты.
        let share = if sell {
            leg.share
        } else if total_out > Decimal::ZERO {
            leg.expected_out / total_out
        } else {
            Decimal::ZERO
        };
        let k = coin_keep(q, &e, coin, side, usdt_dec, taxes.as_ref());
        if share <= Decimal::ZERO || k <= Decimal::ZERO {
            continue;
        }
        let pool_max = if sell {
            max_qty * share * k
        } else {
            max_qty * share / k
        };
        let max = num::to_units(pool_max, dec).unwrap_or(U256::MAX);
        for u in super::breaks_pool::pool(&e, coin, sell, max, limit) {
            let Some(x) = num::to_decimal(u, dec) else {
                continue;
            };
            out.push(if sell { x / (share * k) } else { x * k / share });
        }
        // Решение «335 (лесенка влияния)», ревью 3: ступени лесенки влияния крючка — тоже изломы (quote/breaks_hook.rs).
        let hook = super::breaks_hook::steps(q, &e, side, usdt_dec, (share, k), dec);
        out.extend(hook.into_iter().filter(|x| *x <= max_qty));
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Что делает: доля, которая остаётся у монеты на её шаге: продажа — keep налога продажи; покупка — keep налога
/// покупки × keep крючка (route_calc.rs). Принимает: окружение, пул, монету, сторону, decimals USDT, налоги монеты.
/// Возвращает: keep; пул не допущен или налоги не известны — без этой части (1). Ошибок нет.
fn coin_keep(
    q: &Quoting,
    e: &Entry,
    coin: Address,
    side: Side,
    usdt_dec: u8,
    taxes: Option<&TokenTaxes>,
) -> Decimal {
    let Ok(u) = super::select::usable(q, e, side, usdt_dec) else {
        return Decimal::ONE;
    };
    let zero = TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    };
    let tax = super::pool_tax::step_pct(e, &u, coin, side, taxes.unwrap_or(&zero));
    let hook = if side == Side::Buy {
        keep(u.hook_pct)
    } else {
        Decimal::ONE
    };
    keep(tax) * hook
}
