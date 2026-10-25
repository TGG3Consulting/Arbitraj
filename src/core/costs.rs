//! Файл: core/costs.rs
//! Что делает: полный расчёт одного направления на объём Q (шаг 4 ТЗ, решение 35): котировки обеих
//! сторон по реальной ликвидности, стоимость вывода с A (биржа — комиссия вывода; DEX — газ перевода
//! и налог на перевод), минимумы вывода и минимальный депозит B (неизвестен — сделки нет), доля
//! глубины B (коэффициент h), возврат USDT в каждой сделке (решение 34), запас на движение цены
//! r = k·σ·√T. Всё переводится в USDT (convert.rs). Затраты в монете уменьшают количество, а не сумму.
//! Каждая строка несёт шаг маршрута обмена (решение 52) — экран группирует расчёт по шагам.
//! Решение 66: отказ несёт вид (size.rs Fail: меньше минимума / больше возможного / невозможно) — по нему подбор
//! объёма двигает границы; продажа на бирже B — количество вниз до шага пары (остаток — строка StepRemainder, в
//! выручку не входит); пределы ордера биржи (minQty/maxQty, minNotional/maxNotional) — по худшей цене ордера
//! (costs_quote.rs); списание по бюджету spend_max — количество × худшая цена покупки; комиссии вывода и возврата
//! биржи — из справочника сетей ядра, без запросов к бирже (costs_withdraw.rs).
//! Типы расчёта (Calc, CostLine) — costs_types.rs.
//! Кто вызывает: core/size_curve.rs (подбор объёма), core/partial.rs.
//! От чего зависит: core/types, convert.rs, costs_quote.rs, costs_withdraw.rs, dirctx.rs, size.rs, engine.rs (шлюз).
//! Настройки: через dirctx.rs (profit.*, usdt_return.mode).

use chrono::Utc;
use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{Cost, CostKind, Side};

use super::convert;
use super::costs_quote::{floor_step, order_limits, quote, worst};
pub use super::costs_types::{Calc, CostLine};
use super::costs_withdraw::{floor_precision, return_b, withdraw_a};
use super::dirctx::DirCtx;
use super::engine::Core;
use super::size::Fail;

/// Полный расчёт на объём qty. Принимает: ядро (шлюз, стаканы для худшей цены), контекст направления, объём.
/// Ошибки: Fail с видом и текстом причины (виден на «Живом экране» и в журнале).
pub async fn evaluate(core: &Core, cx: &DirCtx, qty: Decimal) -> Result<Calc, Fail> {
    let deps = &core.inner.deps;
    let d = &cx.dir;
    let coin = d.asset.ticker.clone();
    let qa = quote(core, d, false, qty).await?;
    // Списание по бюджету — по худшей цене покупки (лимит биржи, допуск DEX): столько может уйти или замёрзнуть.
    let worst_a = worst(core, &d.buy_on, &d.asset, Side::Buy, qty, qa.avg_price)?;
    order_limits(&d.buy_on.0, cx.pair_a.as_ref(), qty, worst_a)?;
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
    let wc = withdraw_a(deps, cx, to_send).await?;
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
    quote(core, d, true, sell_qty / cx.h)
        .await
        .map_err(|e| match e {
            Fail::TooLarge(t) => Fail::TooLarge(format!("доля глубины B (h = {}): {t}", cx.h)),
            e => e,
        })?;
    let qb = quote(core, d, true, sell_qty).await?;
    let worst_b = worst(
        core,
        &d.sell_on,
        &d.asset,
        Side::Sell,
        sell_qty,
        qb.avg_price,
    )?;
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
        let rc = return_b(deps, cx, proceeds).await?;
        lines.extend(
            rc.into_iter()
                .map(|c| (convert::usdt_return(c), "возврат USDT")),
        );
    }
    // Запас r = k·σ·√T от выручки (ТЗ, шаг 4).
    let sqrt_t = cx
        .transit_s
        .sqrt()
        .ok_or_else(|| Fail::Stop("время в пути не вычисляется".into()))?;
    let reserve = cx.k * cx.sigma * sqrt_t * proceeds;
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
    let rates = convert::rates(deps, &cx.cex, cx.quote.clone(), &coin, qa.avg_price, &must).await?;
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
        spend_max: qty * worst_a,
        size: None,
    })
}
