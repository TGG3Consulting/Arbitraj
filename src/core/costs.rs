//! Файл: core/costs.rs
//! Что делает: полный расчёт одного направления на объём Q (шаг 4 ТЗ, решение 35): котировки обеих
//! сторон по реальной ликвидности, стоимость вывода с A (биржа — комиссия вывода; DEX — газ перевода
//! и налог на перевод), минимумы вывода и минимальный депозит B (неизвестен — сделки нет), доля
//! глубины B (коэффициент h), возврат USDT в каждой сделке (решение 34), запас на движение цены
//! r = k·σ·√T. Всё переводится в USDT (convert.rs). Затраты в монете уменьшают количество, а не сумму.
//! Каждая строка несёт шаг маршрута обмена (решение 52) — экран группирует расчёт по шагам.
//! Кто вызывает: core/profit.rs (подбор объёма), позже executor.rs (предпроверка).
//! От чего зависит: core/types, convert.rs, dirctx.rs, deps.rs (шлюз).
//! Настройки: через dirctx.rs (profit.*, usdt_return.mode).

use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, MathematicalOps};
use serde::Serialize;

use crate::core::types::{Cost, CostKind, Direction, Quote, RouteStep, Side};

use super::convert;
use super::deps::Deps;
use super::dirctx::DirCtx;

/// Одна строка затрат расчёта.
#[derive(Debug, Clone, Serialize)]
pub struct CostLine {
    pub kind: CostKind,
    /// Где возникла: «A» (покупка), «перевод», «B» (продажа), «возврат USDT», «ядро» (запас r).
    pub stage: &'static str,
    pub amount: Decimal,
    pub asset: String,
    /// В USDT по живым ценам. None — только у затраты «в цене» без живой цены её валюты (показ;
    /// в итог она не входит — уже в средней цене котировки).
    pub usdt: Option<Decimal>,
    /// Уже в средней цене котировки — повторно не вычитается.
    pub in_price: bool,
    /// Затрата в монете: уменьшает количество (до продажи доходит меньше) — повторно не вычитается.
    pub in_qty: bool,
    /// Шаг маршрута обмена (решение 52): «USDT → BNB», «BNB → монета»; None — не шаг обмена.
    pub step: Option<RouteStep>,
}

/// Расчёт направления на объём.
#[derive(Debug, Clone, Serialize)]
pub struct Calc {
    pub direction: Direction,
    /// Сколько покупаем на A.
    pub qty: Decimal,
    /// Сколько дойдёт до B и будет продано.
    pub arrive: Decimal,
    pub buy_avg: Decimal,
    pub sell_avg: Decimal,
    /// Вложено: qty × buy_avg.
    pub invested: Decimal,
    /// Выручка: arrive × sell_avg.
    pub proceeds: Decimal,
    pub costs: Vec<CostLine>,
    /// Затраты, вычитаемые из прибыли (не в цене и не в количестве), USDT.
    pub costs_usdt: Decimal,
    pub profit_usd: Decimal,
    pub profit_pct: Decimal,
    pub transit_s: Decimal,
    pub sigma: Decimal,
    /// Котировки — для худшей цены ордеров и маршрута DEX при исполнении.
    pub buy_quote: Quote,
    pub sell_quote: Quote,
    pub at: DateTime<Utc>,
}

/// Котировка с проверкой глубины. Ошибки: шлюз, глубины не хватает.
async fn quote(deps: &Deps, d: &Direction, sell: bool, qty: Decimal) -> Result<Quote, String> {
    let (venue, side) = if sell {
        (&d.sell_on, Side::Sell)
    } else {
        (&d.buy_on, Side::Buy)
    };
    let v = deps.gw.get(venue).await.map_err(|e| e.message)?;
    let q = v
        .quote(&d.asset, side, qty)
        .await
        .map_err(|e| format!("{}: {}", venue.0, e.message))?;
    if q.fillable_qty < qty {
        return Err(format!(
            "{}: глубины хватает на {} из {qty}",
            venue.0, q.fillable_qty
        ));
    }
    Ok(q)
}

/// Полный расчёт на объём qty. Ошибки: текст причины отказа (видна на «Живом экране» и в журнале).
pub async fn evaluate(deps: &Deps, cx: &DirCtx, qty: Decimal) -> Result<Calc, String> {
    let d = &cx.dir;
    let coin = d.asset.ticker.clone();
    let qa = quote(deps, d, false, qty).await?;
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
    // Изменение ликвидности к моменту продажи: используем только долю h глубины B.
    quote(deps, d, true, arrive / cx.h)
        .await
        .map_err(|e| format!("доля глубины B (h = {}): {e}", cx.h))?;
    let qb = quote(deps, d, true, arrive).await?;
    lines.extend(qb.costs.iter().map(|c| (c.clone(), "B")));
    let proceeds = arrive * qb.avg_price;
    if cx.return_per_trade {
        let b = deps.gw.get(&d.sell_on).await.map_err(|e| e.message)?;
        let rc = b
            .withdraw_cost(&cx.usdt, &cx.net_b, proceeds)
            .await
            .map_err(|e| format!("{}: стоимость возврата USDT: {}", d.sell_on.0, e.message))?;
        lines.extend(rc.into_iter().map(|mut c| {
            c.kind = CostKind::UsdtReturn;
            (c, "возврат USDT")
        }));
    }
    // Запас r = k·σ·√T от выручки (ТЗ, шаг 4).
    let sqrt_t = cx.transit_s.sqrt().ok_or("время в пути не вычисляется")?;
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
        let in_qty = convert::in_coin(&c, &coin) && stage != "B";
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
    })
}
