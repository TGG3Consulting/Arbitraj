//! Файл: core/size_params.rs
//! Что делает: параметры подбора объёма (решение 66) для кривой ядра — вынесено из size_curve.rs (ревью t159: файл
//! перерос 200 строк) без изменений логики до t159: шаг количества пары биржи (A, если A — биржа, иначе B), изломы —
//! накопленные объёмы уровней стакана A (asks) и B (bids), настройки size.*; решение 217 — по снимку рынка (src),
//! решение 239 — настройки снимка; решение 336 — короб с повторным использованием у направления с DEX всегда (решение
//! 363, в логику: была dex.quote_reuse).
//! Задача t159 (ревью t159 Б3): участки допустимости продажи по стакану B (size_scan.rs regions, в логике), признаки
//! «бюджет монотонен» (A — биржа) и «есть сторона DEX»; size.scan_above_pct (size_extra.rs).
//! Кто вызывает: core/size_curve.rs (реэкспорт: profit.rs, card_max.rs, uncapped*.rs, тесты).
//! От чего зависит: dirctx.rs, engine.rs (рынок), frozen.rs, size.rs, size_par.rs, size_scan.rs, size_extra.rs,
//! settings.
//! Настройки: size.bisect_steps, size.grid_points, size.refine_steps, size.tolerance_pct, size.max_evals (общие; значений
//! нет — не задана хоть одна — подбора нет, сделки нет); size.parallel_points, size.bisect_probes (необязательные,
//! решение 336); size.scan_above_pct (необязательная, решение 343).

use rust_decimal::Decimal;

use crate::core::types::{OrderBook, PriceLevel};
use crate::settings::read::At;
use crate::settings::store::Store;

use super::dirctx::DirCtx;
use super::engine::{Core, rd};
use super::frozen::Frozen;
use super::size::Params;

/// Параметры поиска. Принимает: ядро, контекст направления, бюджет, оценку цены покупки (> 0), снимок рынка (решение
/// 217: изломы — по стаканам снимка; None — живые стаканы ядра).
/// Ошибки: текст — не задана настройка size.*, шаг количества неизвестен (ни одна сторона не биржа).
pub fn params(
    core: &Core,
    cx: &DirCtx,
    budget: Decimal,
    price: Decimal,
    src: Option<&Frozen>,
) -> Result<Params, String> {
    let (step, sell_grid) = match (&cx.pair_a, &cx.pair_b) {
        (Some(a), _) => (a.step_size, None),
        (None, Some(b)) => {
            super::size_unit_grid::SellGrid::for_dex_buy(cx.units_b_per_a, b.step_size)?
        }
        _ => return Err("шаг количества неизвестен: ни одна сторона сделки не биржа".into()),
    };
    let d = &cx.dir;
    // Решение 211: стаканы — по тикеру площадки (Presence::ticker стороны направления).
    let (ta, tb) = (&cx.a.ticker, &cx.b.ticker);
    let cum = |b: Option<&OrderBook>, buy: bool| -> Vec<Decimal> {
        b.map(|b| cumulative(if buy { &b.asks } else { &b.bids }))
            .unwrap_or_default()
    };
    // Решение 239: у снимка со входами момента решения — настройки на тот момент.
    let s = src
        .and_then(|f| f.decided.as_deref())
        .map_or(&core.inner.deps.settings, |x| &x.settings);
    // Решение 343 (в логике, ревью t159 Б3): участки допустимости продажи по minNotional / maxNotional стакана B.
    let regions = |b: Option<&OrderBook>| -> Vec<super::size_scan::Run> {
        b.map(|b| super::size_scan::regions(&b.bids, cx.pair_b.as_ref()))
            .unwrap_or_default()
    };
    let (buy_breaks, sell_breaks, sell_regions) = {
        let market = rd(&core.inner.market);
        match src {
            Some(f) => (
                cum(f.book(false), true),
                cum(f.book(true), false),
                regions(f.book(true)),
            ),
            None => (
                cum(market.book(&d.buy_on, ta), true),
                cum(market.book(&d.sell_on, tb), false),
                regions(market.book(&d.sell_on, tb)),
            ),
        }
    };
    let mut p = search_params(s, step, budget, price, buy_breaks, sell_breaks)?;
    if src.is_none() && let Some(qty)=super::signal_probe::measured_share(core,d,cx.units_b_per_a)? {
        p.guess=qty;
        p.buy_breaks.push(qty);
    }
    p.ext.sell_grid = sell_grid;
    if src.is_none()
        && cx.pair_a.is_none()
        && cx.pair_b.is_some()
        && cx.units_b_per_a.is_identity()
        && cx.a.info.is_none()
        && let Some(token) = d.asset.contract
        && let Some(check) = rd(&core.inner.market).coin_check(&d.buy_on, token)
        && let crate::core::types::CheckState::Known(taxes) = &check.taxes
    {
        p.ext.plateau = super::size_plateau::Arrival::new(Decimal::ZERO, taxes.transfer_pct);
    }
    // Решения 336, 363 (в логику): повторное использование в подборе — у направления с DEX (сторона без правил пары
    // биржи, dirctx_venue.rs pair_rules: None — не биржа). Биржа ↔ биржа — короба ради reuse нет (DEX нет, ревью t157
    // Б1); цикл DEX ↔ DEX строит свои Params без reuse (cycle_plan.rs).
    p.reuse = cx.pair_a.is_none() || cx.pair_b.is_none();
    p.ext.sell_regions = sell_regions;
    // Ревью t159 Б3: A — биржа ⇒ списание монотонно (худшая цена asks не убывает); сторона без пары — DEX.
    p.ext.budget_monotone = cx.pair_a.is_some();
    p.ext.no_formula = cx.pair_a.is_none() || cx.pair_b.is_none();
    Ok(p)
}

/// Что делает: параметры поиска при известных шаге количества и изломах (решение 218: общий кусок params и пересчёта
/// по окнам). Принимает: настройки, шаг, бюджет, оценку цены покупки (> 0), изломы покупки и продажи.
/// Возвращает: Params. Ошибки: текст — не задана настройка size.*.
pub fn search_params(
    s: &Store,
    step: Decimal,
    budget: Decimal,
    price: Decimal,
    buy_breaks: Vec<Decimal>,
    sell_breaks: Vec<Decimal>,
) -> Result<Params, String> {
    let int = |name: &str| -> Result<u32, String> {
        let v = s.integer(name, At::default()).map_err(|e| e.message)?;
        u32::try_from(v).map_err(|_| format!("{name} = {v}: вне пределов"))
    };
    let tolerance_pct = s
        .decimal("size.tolerance_pct", At::default())
        .map_err(|e| e.message)?;
    // Решение 336: необязательные; не заданы — 1 (как до 336).
    let (par_points, bisect_probes, par_blocked) = super::size_par::par_settings(s)?;
    Ok(Params {
        step,
        budget,
        guess: budget / price,
        buy_breaks,
        sell_breaks,
        bisect_steps: int("size.bisect_steps")?,
        grid_points: int("size.grid_points")?,
        refine_steps: int("size.refine_steps")?,
        max_evals: int("size.max_evals")?,
        tolerance_pct,
        par_points,
        bisect_probes,
        // Решение 337: пол раннего выхода ставит вызывающий (profit.rs / ниже) — здесь без него.
        early_floor: None,
        reuse: false,
        par_blocked,
        // Задача t159: число точек выше бисекции — здесь; участки — params; пороги, изломы и срок — profit.rs.
        ext: super::size_extra::Extra {
            above_pct: super::size_extra::flags(s).above_pct,
            ..Default::default()
        },
    })
}

/// Накопленные объёмы уровней от лучшего (изломы средней цены). Ошибок нет.
pub fn cumulative(levels: &[PriceLevel]) -> Vec<Decimal> {
    let mut cum = Decimal::ZERO;
    levels
        .iter()
        .map(|l| {
            cum += l.qty;
            cum
        })
        .collect()
}
