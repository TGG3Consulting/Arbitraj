//! Файл: core/costs.rs
//! Что делает: полный расчёт одного направления на объём Q (шаг 4 ТЗ, решение 35): котировки обеих
//! сторон по реальной ликвидности, стоимость вывода с A (биржа — комиссия вывода; DEX — газ перевода
//! и налог на перевод), минимумы вывода и минимальный депозит B (неизвестен — сделки нет), доля
//! глубины B (коэффициент h), возврат USDT в каждой сделке (решение 34), запас на движение цены
//! r = k·σ·√T. Всё переводится в USDT (convert.rs). Затраты в монете уменьшают количество, а не сумму.
//! Каждая строка несёт шаг маршрута обмена (решение 52) — экран группирует расчёт по шагам.
//! Решение 66: отказ несёт вид (size.rs Fail: меньше минимума / больше возможного / невозможно) — по нему подбор
//! объёма двигает границы; продажа на бирже B — количество вниз до шага пары (остаток — строка StepRemainder, в
//! выручку не входит); сумма ордера каждой биржи — в пределах minNotional / maxNotional пары (costs_quote.rs).
//! Типы расчёта (Calc, CostLine) — costs_types.rs.
//! Кто вызывает: core/size_curve.rs (подбор объёма), core/partial.rs.
//! От чего зависит: core/types, convert.rs, costs_quote.rs, dirctx.rs, size.rs, deps.rs (шлюз).
//! Настройки: через dirctx.rs (profit.*, usdt_return.mode).

use chrono::Utc;
use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{Cost, CostKind};

use super::convert;
use super::costs_quote::{floor_step, notional, quote};
pub use super::costs_types::{Calc, CostLine};
use super::deps::Deps;
use super::dirctx::DirCtx;
use super::size::Fail;

/// Полный расчёт на объём qty. Ошибки: Fail с видом и текстом причины (виден на «Живом экране» и в журнале).
pub async fn evaluate(deps: &Deps, cx: &DirCtx, qty: Decimal) -> Result<Calc, Fail> {
    let d = &cx.dir;
    let coin = d.asset.ticker.clone();
    let qa = quote(deps, d, false, qty).await?;
    notional(&d.buy_on.0, cx.pair_a.as_ref(), qty * qa.avg_price)?;
    let a = deps.gw.get(&d.buy_on).await.map_err(|e| e.message)?;
    // Затраты покупки в монете (комиссия биржи в монете) уменьшают выводимое количество.
    let mut lines: Vec<(Cost, &'static str)> = qa.costs.iter().map(|c| (c.clone(), "A")).collect();
    let coin_fee = |cs: &[Cost]| -> Decimal {
        cs.iter()
            .filter(|c| !c.in_price && convert::in_coin(c, &coin))
            .map(|c| c.amount)
            .sum()
    };
    let to_send = qty - coin_fee(&qa.costs);
    cx.check_withdraw(to_send)?;
    let wc = a
        .withdraw_cost(&d.asset, &cx.net_a, to_send)
        .await
        .map_err(|e| format!("{}: стоимость вывода: {}", d.buy_on.0, e.message))?;
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
    quote(deps, d, true, sell_qty / cx.h)
        .await
        .map_err(|e| match e {
            Fail::TooLarge(t) => Fail::TooLarge(format!("доля глубины B (h = {}): {t}", cx.h)),
            e => e,
        })?;
    let qb = quote(deps, d, true, sell_qty).await?;
    notional(&d.sell_on.0, cx.pair_b.as_ref(), sell_qty * qb.avg_price)?;
    lines.extend(qb.costs.iter().map(|c| (c.clone(), "B")));
    if sell_qty < arrive {
        // Остаток меньше шага остаётся на B непроданным — в выручку не входит (показ, решение 66).
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
        let b = deps.gw.get(&d.sell_on).await.map_err(|e| e.message)?;
        let rc = b
            .withdraw_cost(&cx.usdt, &cx.net_b, proceeds)
            .await
            .map_err(|e| format!("{}: стоимость возврата USDT: {}", d.sell_on.0, e.message))?;
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
        size: None,
    })
}
