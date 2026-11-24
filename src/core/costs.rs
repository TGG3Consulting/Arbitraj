//! Файл: core/costs.rs
//! Что делает: полный расчёт одного направления на объём Q (шаг 4 ТЗ, решение 35): котировки обеих
//! сторон по реальной ликвидности, стоимость вывода с A (биржа — комиссия вывода; DEX — газ перевода
//! и налог на перевод), минимумы вывода и минимальный депозит B (неизвестен — сделки нет), доля
//! глубины B (коэффициент h), возврат USDT в каждой сделке (решение 34), запас на движение цены
//! r = выручка·(1 − e^(−k·σ·√T)) (решение 390: не больше выручки; решение 240: не меньше доли закрытия разницы
//! profit.closure_share — reserve.rs). Всё переводится в USDT
//! (convert.rs). Затраты в монете уменьшают количество, а не сумму.
//! Каждая строка несёт шаг маршрута обмена (решение 52) — экран группирует расчёт по шагам.
//! Решение 66: отказ несёт вид (size.rs Fail: меньше минимума / больше возможного / невозможно) — по нему подбор
//! объёма двигает границы; продажа на бирже B — количество вниз до шага пары (остаток — строка StepRemainder, в
//! выручку не входит); пределы ордера биржи (minQty/maxQty, minNotional/maxNotional) — по худшей цене ордера
//! (costs_quote.rs); списание по бюджету spend_max — количество × худшая цена покупки плюс комиссия покупки в
//! котируемой монете по худшей цене (решение 219, costs_types.rs quote_fee_at_worst); худшая цена покупки — в план
//! (Calc::buy_worst, решение 262: лимит ордера покупки на бирже); комиссии вывода и возврата
//! биржи — из справочника сетей ядра, без запросов к бирже (costs_withdraw.rs).
//! Типы расчёта (Calc, CostLine) — costs_types.rs. Решение 129: каждый запрос к шлюзу — часть замера этапа
//! (calc_timing.rs): котировка A / B с id площадки, стоимость вывода и возврата, курсы затрат; решение 265 — проверка
//! доли глубины B отдельной частью («доля глубины B») — только в подробном замере сделки (calc_detail.rs).
//! Решение 217: evaluate_on — тот же расчёт по снимку рынка момента решения (core/frozen.rs: котировки и худшие цены
//! обеих сторон по снимку) для расчёта «без потолка» по сделке.
//! Решение 218: сама модель — evaluate_with над источником данных (costs_source.rs Source): бой, кандидаты и снимок —
//! источник Live (живое ядро); пересчёт старых записей «без потолка» по окнам — свой источник (uncapped_replay.rs).
//! Второй модели затрат нет.
//! Решение 246: evaluate_capped — с бюджетом подбора объёма: списание сверх бюджета — отказ сразу после стороны A
//! (котировки B, вывод, возврат и курсы не считаются); итог в бюджете — тот же Calc, что у evaluate_with.
//! Решение 250: проверка доли глубины h — Source::fillable (исполнимость без цены; у bsc_dex — без полной котировки,
//! если лучший одиночный пул исполняет объём целиком: quote/fills.rs).
//! Решение 392 (t180): худшие цены ордеров A и B — внутри ценового коридора биржи (DirCtx.band_a / band_b, прочитан
//! перед покупкой — price_band.rs): за коридором «дальше» — Fail::TooLarge, подбор берёт меньший объём.
//! Решение 358 (только журнал): при сравнении σ в контексте (DirCtx::sigma_check) — запас и прибыль по σ на горизонте T
//! рядом с прежними (reserve_check.rs priced); прибыль и решение — по прежнему запасу.
//! Кто вызывает: core/size_curve.rs (подбор объёма: CoreCurve — evaluate_capped, SourceCurve), core/partial.rs.
//! От чего зависит: core/types, convert.rs, costs_quote.rs, costs_source.rs, costs_withdraw.rs, dirctx.rs, precheck_dex_cut.rs (решение 380: предел продажи), reserve.rs,
//! reserve_check.rs, size.rs, engine.rs (шлюз), price_band.rs (решение 392).
//! Настройки: через dirctx.rs (profit.*, usdt_return.mode).

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::Side;

use super::calc_timing::stage;
use super::costs_quote::order_limits;
pub use super::costs_source::{Live, Source};
pub use super::costs_types::{Calc, CostLine};
use super::dirctx::DirCtx;
use super::engine::Core;
use super::frozen::Frozen;
use super::size::Fail;

/// Полный расчёт на объём qty. Принимает: ядро (шлюз, стаканы для худшей цены), контекст направления, объём.
/// Ошибки: Fail с видом и текстом причины (виден на «Живом экране» и в журнале).
pub async fn evaluate(core: &Core, cx: &DirCtx, qty: Decimal) -> Result<Calc, Fail> {
    evaluate_on(core, cx, qty, None).await
}

/// То же, что evaluate, по снимку рынка момента решения (решение 217, core/frozen.rs): котировки и худшие цены обеих
/// сторон — по снимку; src None — живые. Принимает: ядро, контекст, объём, снимок. Ошибки: как у evaluate.
pub async fn evaluate_on(
    core: &Core,
    cx: &DirCtx,
    qty: Decimal,
    src: Option<&Frozen>,
) -> Result<Calc, Fail> {
    evaluate_with(cx, qty, &Live { core, src }).await
}

/// Что делает: полный расчёт на объём qty над источником данных (решение 218, costs_source.rs) — одна модель для боя,
/// снимка и пересчёта по окнам. Принимает: контекст направления, объём, источник. Возвращает: Calc.
/// Ошибки: Fail с видом и текстом причины (как у evaluate).
pub async fn evaluate_with<S: Source>(cx: &DirCtx, qty: Decimal, src: &S) -> Result<Calc, Fail> {
    evaluate_capped(cx, qty, src, None).await
}

/// Чистая будущая выручка от перевода уже купленного остатка. Количество и цена взяты из исполнения.
/// Вывод, продажа, возврат и риск используют те же источники и ограничения, что и обычный план.
pub async fn held_value<S: Source>(
    cx: &DirCtx,
    qty: Decimal,
    buy_avg: Decimal,
    src: &S,
) -> Result<Decimal, Fail> {
    let sale = super::costs_exit::calculate(cx, src, qty, buy_avg, Vec::new(), None).await?;
    Ok(sale.proceeds - sale.costs_usdt)
}

/// Что делает: то же, что evaluate_with, с бюджетом подбора объёма (решение 246): списание по бюджету (spend_max)
/// известно уже после стороны A — объём сверх бюджета подбор (size_memo.rs) всё равно отбросит, поэтому сторона B
/// (котировки DEX — самые дорогие), вывод, возврат и курсы на нём не считаются. Отказ — тот же, что у size_memo.rs
/// (over_budget: вид OverBudget, тот же текст). Принимает: как evaluate_with и бюджет (None — без проверки, как
/// evaluate_with). Возвращает: Calc. Ошибки: как у evaluate_with; OverBudget — списание больше бюджета.
pub async fn evaluate_capped<S: Source>(
    cx: &DirCtx,
    qty: Decimal,
    src: &S,
    budget: Option<Decimal>,
) -> Result<Calc, Fail> {
    evaluate_observed(cx, qty, src, budget, None).await
}

pub(super) async fn evaluate_observed<S: Source>(
    cx: &DirCtx,
    qty: Decimal,
    src: &S,
    budget: Option<Decimal>,
    mut depth: Option<&mut super::final_depth::Observation>,
) -> Result<Calc, Fail> {
    let d = &cx.dir;
    let qa = stage(
        || format!("котировка A {}", d.buy_on.0),
        src.quote(d, false, qty),
    )
    .await?;
    super::final_depth::check(
        &d.buy_on.0,
        super::final_depth::Leg::Buy,
        qty,
        qa.fillable_qty,
        depth.as_deref_mut(),
    )?;
    // Списание по бюджету — по худшей цене покупки (лимит биржи, допуск DEX): столько может уйти или замёрзнуть.
    let worst_a = src.worst(d, false, qty, qa.avg_price)?;
    order_limits(&d.buy_on.0, cx.pair_a.as_ref(), qty, worst_a)?;
    // Решение 392 (t180): лимит покупки внутри ценового коридора биржи A (прочитан перед покупкой; иначе — None).
    super::price_band::check(&d.buy_on.0, cx.band_a.as_deref(), Side::Buy, worst_a)?;
    // Решение 246: та же формула списания, что в итоге ниже (spend_max), — до стороны B.
    if let Some(b) = budget {
        let spend =
            qty * worst_a + super::costs_types::quote_fee_at_worst(&qa, &cx.quote[0], worst_a);
        if spend > b {
            return Err(super::size_memo::over_budget(qty, spend, b));
        }
    }
    let lines = qa.costs.iter().map(|c| (c.clone(), "A")).collect();
    let held_qty = qty - super::costs_exit::coin_fee(&qa.costs, &d.asset.ticker);
    let super::costs_exit::Sale {
        sent_a,
        arrive,
        sell_qty,
        qb,
        proceeds,
        costs,
        costs_usdt,
        rates,
    } = super::costs_exit::calculate(cx, src, held_qty, qa.avg_price, lines, depth).await?;
    let invested = qty * qa.avg_price;
    // Решение 219: комиссия покупки в котируемой монете A по худшей цене — часть списания (MEXC 30004).
    let spend_fee = super::costs_types::quote_fee_at_worst(&qa, &cx.quote[0], worst_a);
    let profit = proceeds - invested - costs_usdt;
    // Решение 358 (только журнал): запас и прибыль по σ на горизонте T рядом с прежними; решение — по прежним.
    // Не-async функция (reserve_check.rs priced_box): значения сравнения не живут в кадре этого будущего.
    let sigma_check =
        super::reserve_check::priced_box(cx, &costs, proceeds, &qa, &qb, profit, &rates);
    Ok(Calc {
        direction: d.clone(),
        units_b_per_a: cx.units_b_per_a,
        transfer_sent_a: (cx.units_b_per_a != super::settlement_unit::UnitRatio::IDENTITY)
            .then_some(sent_a),
        qty,
        arrive,
        sell_qty,
        buy_avg: qa.avg_price,
        sell_avg: qb.avg_price,
        invested,
        proceeds,
        costs,
        costs_usdt,
        profit_usd: profit,
        profit_pct: if invested.is_zero() {
            Decimal::ZERO
        } else {
            profit / invested * Decimal::ONE_HUNDRED
        },
        transit_s: cx.transit_s,
        sigma: cx.sigma,
        sigma_check,
        buy_quote: qa,
        sell_quote: qb,
        at: Utc::now(),
        spend_max: qty * worst_a + spend_fee,
        // Решение 262: худший уровень покупки плана — лимит ордера покупки на бирже (executor.rs).
        buy_worst: worst_a,
        buy_band: cx.band_a.is_some(),
        spend_fee,
        size: None,
    })
}
