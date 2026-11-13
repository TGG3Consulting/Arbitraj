//! Файл: core/costs.rs
//! Что делает: полный расчёт одного направления на объём Q (шаг 4 ТЗ, решение 35): котировки обеих
//! сторон по реальной ликвидности, стоимость вывода с A (биржа — комиссия вывода; DEX — газ перевода
//! и налог на перевод), минимумы вывода и минимальный депозит B (неизвестен — сделки нет), доля
//! глубины B (коэффициент h), возврат USDT в каждой сделке (решение 34), запас на движение цены
//! r = k·σ·√T (решение 240: не меньше доли закрытия разницы profit.closure_share — reserve.rs). Всё переводится в USDT
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
//! доли глубины B отдельной частью («доля глубины B»).
//! Решение 217: evaluate_on — тот же расчёт по снимку рынка момента решения (core/frozen.rs: котировки и худшие цены
//! обеих сторон по снимку) для расчёта «без потолка» по сделке.
//! Решение 218: сама модель — evaluate_with над источником данных (costs_source.rs Source): бой, кандидаты и снимок —
//! источник Live (живое ядро); пересчёт старых записей «без потолка» по окнам — свой источник (uncapped_replay.rs).
//! Второй модели затрат нет.
//! Решение 246: evaluate_capped — с бюджетом подбора объёма: списание сверх бюджета — отказ сразу после стороны A
//! (котировки B, вывод, возврат и курсы не считаются); итог в бюджете — тот же Calc, что у evaluate_with.
//! Решение 250: проверка доли глубины h — Source::fillable (исполнимость без цены; у bsc_dex — без полной котировки,
//! если лучший одиночный пул исполняет объём целиком: quote/fills.rs).
//! Кто вызывает: core/size_curve.rs (подбор объёма: CoreCurve — evaluate_capped, SourceCurve), core/partial.rs.
//! От чего зависит: core/types, convert.rs, costs_quote.rs, costs_source.rs, costs_withdraw.rs, dirctx.rs, reserve.rs, size.rs,
//! engine.rs (шлюз).
//! Настройки: через dirctx.rs (profit.*, usdt_return.mode).

use chrono::Utc;
use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{Cost, CostKind};

use super::calc_timing::stage;
use super::convert;
use super::costs_quote::{enough, enough_qty, floor_step, order_limits};
pub use super::costs_source::{Live, Source};
pub use super::costs_types::{Calc, CostLine};
use super::costs_withdraw::floor_precision;
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

/// Что делает: то же, что evaluate_with, с бюджетом подбора объёма (решение 246): списание по бюджету (spend_max)
/// известно уже после стороны A — объём сверх бюджета подбор (size_memo.rs) всё равно отбросит, поэтому сторона B
/// (котировки DEX — самые дорогие), вывод, возврат и курсы на нём не считаются. Отказ — тот же, что у size_memo.rs
/// (over_budget: вид TooLarge, тот же текст). Принимает: как evaluate_with и бюджет (None — без проверки, как
/// evaluate_with). Возвращает: Calc. Ошибки: как у evaluate_with; TooLarge — списание больше бюджета.
pub async fn evaluate_capped<S: Source>(
    cx: &DirCtx,
    qty: Decimal,
    src: &S,
    budget: Option<Decimal>,
) -> Result<Calc, Fail> {
    let d = &cx.dir;
    let coin = d.asset.ticker.clone();
    let qa = stage(
        || format!("котировка A {}", d.buy_on.0),
        src.quote(d, false, qty),
    )
    .await
    .and_then(|q| enough(&d.buy_on.0, q, qty))?;
    // Списание по бюджету — по худшей цене покупки (лимит биржи, допуск DEX): столько может уйти или замёрзнуть.
    let worst_a = src.worst(d, false, qty, qa.avg_price)?;
    order_limits(&d.buy_on.0, cx.pair_a.as_ref(), qty, worst_a)?;
    // Решение 246: та же формула списания, что в итоге ниже (spend_max), — до стороны B.
    if let Some(b) = budget {
        let spend =
            qty * worst_a + super::costs_types::quote_fee_at_worst(&qa, &cx.quote[0], worst_a);
        if spend > b {
            return Err(super::size_memo::over_budget(qty, spend, b));
        }
    }
    // Затраты покупки в монете (комиссия биржи в монете) уменьшают выводимое количество.
    let mut lines: Vec<(Cost, &'static str)> = qa.costs.iter().map(|c| (c.clone(), "A")).collect();
    let coin_fee = |cs: &[Cost]| -> Decimal {
        cs.iter()
            .filter(|c| !c.in_price && convert::in_coin(c, &coin))
            .map(|c| c.amount)
            .sum()
    };
    let to_send = floor_precision(qty - coin_fee(&qa.costs), cx.a.info.as_ref());
    cx.check_withdraw(to_send)?;
    let wc = stage(
        || format!("стоимость вывода {}", d.buy_on.0),
        src.withdraw(cx, to_send),
    )
    .await?;
    let arrive = to_send - coin_fee(&wc);
    lines.extend(wc.into_iter().map(|c| (c, "перевод")));
    cx.check_deposit(arrive)?;
    let sell_qty = floor_step(arrive, cx.pair_b.as_ref());
    if sell_qty <= Decimal::ZERO {
        return Err(Fail::TooSmall(format!(
            "приход {arrive} меньше шага количества B"
        )));
    }
    // Изменение ликвидности к моменту продажи: используем только долю h глубины B.
    // Решение 250: на доле глубины нужна только исполнимость (fillable) — без цены; адаптер может ответить дешевле
    // полной котировки, если точно знает, что объём исполнится целиком (bsc_dex: quote/fills.rs).
    let name_b = || format!("котировка B {}", d.sell_on.0);
    let deep = sell_qty / cx.h;
    // Решение 265: доля глубины — своя часть замера (раньше шла под именем «котировка B»).
    let name_h = || format!("доля глубины B {}", d.sell_on.0);
    stage(name_h, src.fillable(d, true, deep))
        .await
        .and_then(|f| enough_qty(&d.sell_on.0, f, deep))
        .map_err(|e| match e {
            Fail::TooLarge(t) => Fail::TooLarge(format!("доля глубины B (h = {}): {t}", cx.h)),
            e => e,
        })?;
    let qb = stage(name_b, src.quote(d, true, sell_qty))
        .await
        .and_then(|q| enough(&d.sell_on.0, q, sell_qty))?;
    let worst_b = src.worst(d, true, sell_qty, qb.avg_price)?;
    order_limits(&d.sell_on.0, cx.pair_b.as_ref(), sell_qty, worst_b)?;
    lines.extend(qb.costs.iter().map(|c| (c.clone(), "B")));
    if sell_qty < arrive {
        // Остаток меньше шага остаётся на B непроданным (пыль) — в выручку не входит (показ, решение 66).
        let rest = Cost::new(
            CostKind::StepRemainder,
            arrive - sell_qty,
            coin.clone(),
            false,
        );
        lines.push((rest, "B"));
    }
    let proceeds = sell_qty * qb.avg_price;
    if cx.return_per_trade {
        // Решение 118 (Я-22): перегоняется выручка минус комиссия продажи в котируемой монете B — то, что пришло.
        let sell_fee: Decimal = qb
            .costs
            .iter()
            .filter(|c| !c.in_price && c.paid_in.is_none() && c.asset == cx.quote[1])
            .map(|c| c.amount)
            .sum();
        let ret = src.ret(cx, proceeds - sell_fee);
        let rc = stage(|| format!("стоимость возврата USDT {}", d.sell_on.0), ret).await?;
        lines.extend(
            rc.into_iter()
                .map(|c| (convert::usdt_return(c), super::costs_types::RETURN_STAGE)),
        );
    }
    // Запас r = k·σ·√T от выручки (ТЗ, шаг 4); решение 240 — не меньше доли закрытия разницы (reserve.rs).
    let sqrt_t = cx
        .transit_s
        .sqrt()
        .ok_or_else(|| Fail::Stop("время в пути не вычисляется".into()))?;
    let reserve = super::reserve::amount(
        cx.k,
        cx.sigma,
        sqrt_t,
        proceeds,
        cx.closure_share,
        qa.avg_price,
        qb.avg_price,
    );
    lines.push((
        Cost::new(
            CostKind::PriceMoveReserve,
            reserve,
            cx.quote[0].clone(),
            false,
        ),
        "ядро",
    ));
    // Живые цены всех валют затрат (BNB, промежуточные монеты шагов маршрута — решение 52).
    // Цены нужны обязательно для затрат, которые вычитаются; затраты «в цене» — только для показа.
    let must: Vec<Cost> = lines
        .iter()
        .filter(|(c, _)| !c.in_price)
        .map(|(c, _)| c.clone())
        .collect();
    let rates = src.rates(cx, &coin, qa.avg_price, &must);
    let rates = stage(|| "курсы затрат".into(), rates).await?;
    let mut costs = vec![];
    let mut costs_usdt = Decimal::ZERO;
    for (c, stage) in lines {
        // Газ в BNB у сделки с BNB — затрата, а не количество (решение 62, convert::in_coin).
        // Остаток от шага (решение 66) уже вне выручки — только показ.
        let in_qty =
            (convert::in_coin(&c, &coin) && stage != "B") || c.kind == CostKind::StepRemainder;
        let usdt = if c.in_price {
            convert::to_usdt(&c, &rates).ok()
        } else {
            let u = convert::to_usdt(&c, &rates)?;
            if !in_qty {
                costs_usdt += u;
            }
            Some(u)
        };
        costs.push(CostLine {
            kind: c.kind,
            stage,
            amount: c.amount,
            asset: c.asset,
            usdt,
            in_price: c.in_price,
            in_qty,
            step: c.step,
        });
    }
    let invested = qty * qa.avg_price;
    // Решение 219: комиссия покупки в котируемой монете A по худшей цене — часть списания (MEXC 30004).
    let spend_fee = super::costs_types::quote_fee_at_worst(&qa, &cx.quote[0], worst_a);
    let profit = proceeds - invested - costs_usdt;
    Ok(Calc {
        direction: d.clone(),
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
        buy_quote: qa,
        sell_quote: qb,
        at: Utc::now(),
        spend_max: qty * worst_a + spend_fee,
        // Решение 262: худший уровень покупки плана — лимит ордера покупки на бирже (executor.rs).
        buy_worst: worst_a,
        spend_fee,
        size: None,
    })
}
