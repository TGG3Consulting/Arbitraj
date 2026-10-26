//! Файл: core/card_max.rs
//! Что делает: максимальный объём и максимальная прибыль карточки сигнала (решение 74) на живых данных ядра: та же
//! кривая полного расчёта (size_curve.rs → costs.rs, все затраты), что и у сделки, но без бюджета — до верхнего
//! предела поиска card.max_search_usdt; пороги — money.min_profit_usd и money.min_profit_pct (как у сделки, profit.rs).
//! Поиск ограничен тем, что ядро видит: загруженными уровнями стакана (venue.depth_levels) и прочитанной картой пула
//! (котировка DEX) — причина края — в MaxVolume.limit.
//! Кто вызывает: core/card.rs.
//! От чего зависит: size_max.rs, size_curve.rs, dirctx.rs, costs.rs, settings.
//! Настройки: card.max_search_usdt, money.min_profit_usd, money.min_profit_pct, size.* (через size_curve::params).

use rust_decimal::Decimal;

use crate::core::types::Side;
use crate::settings::read::At;

use super::costs::Calc;
use super::dirctx::DirCtx;
use super::engine::{Core, rd};
use super::size_curve::{CoreCurve, params};
use super::size_max::{MaxVolume, max_volume};

/// Что делает: поиск без бюджета для прошедшего кандидата.
/// Принимает: ядро, расчёт на бюджете (его объём — заведомо проходящий). Возвращает: MaxVolume.
/// Ошибки: текст — не задан card.max_search_usdt или порог, контекст направления, расчёт невозможен.
pub async fn compute(core: &Core, calc: &Calc) -> Result<MaxVolume, String> {
    let s = &core.inner.deps.settings;
    let g = |name: &str| s.decimal(name, At::default()).map_err(|e| e.message);
    let cap = g("card.max_search_usdt")?;
    let (min_usd, min_pct) = (g("money.min_profit_usd")?, g("money.min_profit_pct")?);
    let d = &calc.direction;
    let cx = DirCtx::build(core, d)?;
    let price = rd(&core.inner.market)
        .best(&d.buy_on, &d.asset.ticker, Side::Buy)
        .filter(|p| !p.is_zero())
        .unwrap_or(calc.buy_avg);
    if price.is_zero() {
        return Err("нет цены покупки на A".into());
    }
    let p = params(core, &cx, cap, price)?;
    let mut curve = CoreCurve { core, cx: &cx };
    max_volume(&mut curve, &p, calc.qty, |pt| {
        passes(&pt.data, min_usd, min_pct)
    })
    .await
    .map_err(|e| e.to_string())
}

/// Проходит ли расчёт пороги прибыли (как profit.rs). Ошибок нет.
pub fn passes(c: &Calc, min_usd: Decimal, min_pct: Decimal) -> bool {
    c.profit_usd >= min_usd && c.profit_pct >= min_pct
}
