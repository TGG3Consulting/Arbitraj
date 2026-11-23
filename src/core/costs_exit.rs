//! Вывод, продажа и возврат USDT для плана входа и уже купленного остатка (core/costs.rs).
//! На входе количество после комиссии покупки. При планировании строки покупки участвуют в общей
//! конверсии затрат. После исполнения понесённые расходы уже учтены в сделке.

use rust_decimal::Decimal;

use crate::core::types::{Cost, CostKind, Quote, Side};

use super::calc_timing::stage;
use super::convert::{self, Rates};
use super::costs_quote::{floor_step, order_limits};
use super::costs_source::Source;
use super::costs_types::CostLine;
use super::costs_withdraw::floor_precision;
use super::dirctx::DirCtx;
use super::final_depth::{self, Leg};
use super::size::Fail;

/// Денежные и количественные составляющие плана от вывода с A до возврата USDT.
pub(super) struct Sale {
    pub arrive: Decimal,
    pub sell_qty: Decimal,
    pub qb: Quote,
    pub proceeds: Decimal,
    pub costs: Vec<CostLine>,
    pub costs_usdt: Decimal,
    pub rates: Rates,
}

/// Количество монет, удержанных вне цены, с исключением газа и чаевых сети.
pub(super) fn coin_fee(costs: &[Cost], coin: &str) -> Decimal {
    costs
        .iter()
        .filter(|c| !c.in_price && convert::in_coin(c, coin))
        .map(|c| c.amount)
        .sum()
}

pub(super) async fn calculate<S: Source>(
    cx: &DirCtx,
    src: &S,
    held_qty: Decimal,
    buy_avg: Decimal,
    mut lines: Vec<(Cost, &'static str)>,
    mut depth: Option<&mut super::final_depth::Observation>,
) -> Result<Sale, Fail> {
    let d = &cx.dir;
    let coin = d.asset.ticker.clone();
    let to_send = floor_precision(held_qty, cx.a.info.as_ref());
    cx.check_withdraw(to_send)?;
    let wc = stage(
        || format!("стоимость вывода {}", d.buy_on.0),
        src.withdraw(cx, to_send),
    )
    .await?;
    let arrive = to_send - coin_fee(&wc, &coin);
    lines.extend(wc.into_iter().map(|c| (c, "перевод")));
    cx.check_deposit(arrive)?;
    let sell_qty = floor_step(arrive, cx.pair_b.as_ref());
    if sell_qty <= Decimal::ZERO {
        return Err(Fail::TooSmall(format!(
            "приход {arrive} меньше шага количества B"
        )));
    }
    // Решение 380: объём продажи не больше проверяемого симуляцией продажи до покупки (только у пересчёта предпроверки).
    super::precheck_dex_cut::sell_max_ok(cx, sell_qty)?;
    // Изменение ликвидности к моменту продажи: используем только долю h глубины B.
    // Решение 250: на доле глубины нужна только исполнимость (fillable) — без цены; адаптер может ответить дешевле
    // полной котировки, если точно знает, что объём исполнится целиком (bsc_dex: quote/fills.rs).
    let name_b = || format!("котировка B {}", d.sell_on.0);
    let deep = sell_qty / cx.h;
    // Решение 265: доля глубины — своя часть подробного замера сделки (log.step_timing).
    // Только в подробном замере сделки (calc_detail.rs); в замере кандидата — как до решения 265, «котировка B».
    let name_h = || match super::calc_detail::detail() {
        true => format!("доля глубины B {}", d.sell_on.0),
        false => name_b(),
    };
    stage(name_h, src.fillable(d, true, deep))
        .await
        .and_then(|f| {
            final_depth::check(
                &d.sell_on.0,
                Leg::Reserve { arrive },
                deep,
                f,
                depth.as_deref_mut(),
            )
        })
        .map_err(|e| match e {
            Fail::TooLarge(t) => Fail::TooLarge(format!("доля глубины B (h = {}): {t}", cx.h)),
            e => e,
        })?;
    let qb = stage(name_b, src.quote(d, true, sell_qty)).await?;
    final_depth::check(
        &d.sell_on.0,
        Leg::Sell { arrive },
        sell_qty,
        qb.fillable_qty,
        depth,
    )?;
    let worst_b = src.worst(d, true, sell_qty, qb.avg_price)?;
    order_limits(&d.sell_on.0, cx.pair_b.as_ref(), sell_qty, worst_b)?;
    super::price_band::check(&d.sell_on.0, cx.band_b.as_deref(), Side::Sell, worst_b)?;
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
    // Запас r = выручка·(1 − e^(−k·σ·√T)) (ТЗ, шаг 4; решение 390 — не больше выручки); решение 240 — не меньше доли
    // закрытия разницы (reserve.rs).
    // Доля keep = e^(−k·σ·√T) — одна на контекст (DirCtx.keep, reserve.rs keep_of), не на точку объёма.
    let keep = cx
        .keep
        .ok_or_else(|| Fail::Stop("время в пути не вычисляется".into()))?;
    let reserve = super::reserve::amount(keep, proceeds, cx.closure_share, buy_avg, qb.avg_price);
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
    let rates = src.rates(cx, &coin, buy_avg, &must);
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
    Ok(Sale {
        arrive,
        sell_qty,
        qb,
        proceeds,
        costs,
        costs_usdt,
        rates,
    })
}
