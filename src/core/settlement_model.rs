//! Файл: core/settlement_model.rs.
//! Что делает: считает прибыль полного маршрута в валюте учёта.
//! Кто вызывает: API и выбор исполнимого маршрута.
//! От чего зависит: снимки обеих сторон, комиссии и оценка расходов.
//! Настройки: пара целевой сделки и расчётная пара исходной биржи.
use super::{
    costs_source::{Live, Source},
    settlement,
    settlement_path::Leg,
};
use crate::core::{
    engine::Core,
    types::{Cost, CostKind, Direction, Side},
};
use rust_decimal::Decimal;

pub use super::settlement_values::Model;
use super::settlement_values::{add, mul};

/// Строит обе стороны на текущих данных. Учитывает встречные заявки других сделок через Live,
/// проверенные пределы и удержания перевода токена через общий расчёт quantities.
pub async fn evaluate(core: &Core, d: &Direction, qty: Decimal) -> Result<Model, String> {
    let market = super::settlement_market::Market::direction(core, d)?;
    let dex = market.dex.clone();
    let cex = &market.cex;
    let funding = d.buy_on == dex;
    if qty <= Decimal::ZERO
        || d.asset.contract.is_none()
        || d.asset.contract == market.asset.contract
    {
        return Err("Нужны положительный объём и токен, отличный от расчётного".into());
    }
    let wallet = super::dex_account::of_direction_now(core, d)
        .ok_or("Нет привязанного кошелька этого направления конверсии")?;
    let gw = core
        .inner
        .deps
        .gw
        .get_as(cex, None)
        .await
        .map_err(|e| e.message)?;
    if !gw.supports_settlement_order() || (funding && !gw.supports_settlement_withdrawal()) {
        return Err("Для биржи отсутствует восстанавливаемая конверсия/вывод".into());
    }
    let cx = super::dirctx::DirCtx::for_settlement(core, d)?;
    let src = Live { core, src: None };
    let qa = src.quote(d, false, qty).await.map_err(|e| e.to_string())?;
    if qa.fillable_qty < qty {
        return Err("Покупка основного токена не покрыта ликвидностью".into());
    }
    let worst = src
        .worst(d, false, qty, qa.avg_price)
        .map_err(|e| e.to_string())?;
    super::costs_quote::order_limits(&d.buy_on.0, cx.pair_a.as_ref(), qty, worst)
        .map_err(|e| e.to_string())?;
    super::price_band::check(&d.buy_on.0, cx.band_a.as_deref(), Side::Buy, worst)
        .map_err(|e| e.to_string())?;
    let held = qty - super::costs_exit::coin_fee(&qa.costs, &d.asset.ticker);
    let lines = qa.costs.iter().cloned().map(|c| (c, "A")).collect();
    let exit = super::costs_exit::quantities(&cx, &src, held, lines, None)
        .await
        .map_err(|e| e.to_string())?;
    let worst_b = src
        .worst(d, true, exit.sell_qty, exit.qb.avg_price)
        .map_err(|e| e.to_string())?;
    let net = market.network(core, funding)?;
    let principal = mul(qty, qa.avg_price)?;
    let budget = mul(qty, worst)?;
    let proceeds = mul(exit.sell_qty, exit.qb.avg_price)?;
    let mut lines = exit.lines;
    if !funding {
        let wallet_gw = core
            .inner
            .deps
            .gw
            .get_as(&dex, Some(&wallet))
            .await
            .map_err(|e| e.message)?;
        let fee = wallet_gw
            .withdraw_cost(&net.asset, &cx.net_b, proceeds)
            .await
            .map_err(|e| e.message)?;
        lines.extend(fee.into_iter().map(|c| (c, "возврат")));
    }
    let mut usdc = Decimal::ZERO;
    let mut buy_fee = Decimal::ZERO;
    let mut other = vec![];
    for (cost, stage) in lines {
        if cost.in_price
            || cost.amount.is_zero()
            || cost.kind == CostKind::StepRemainder
            || (super::convert::in_coin(&cost, &d.asset.ticker) && stage != "B")
        {
            continue;
        }
        if cost.amount < Decimal::ZERO || cost.paid_in.is_some() {
            return Err(
                "Нужен подтверждённый дебет монеты скидки или ребейта основной сделки".into(),
            );
        }
        if cost.asset == market.asset.ticker {
            usdc = add(usdc, cost.amount)?;
        } else if cost.asset == market.quote && stage == "A" {
            buy_fee = add(buy_fee, cost.amount)?;
        } else {
            other.push(cost);
        }
    }
    let (conversion, leg, invested, capital, final_quote) = if funding {
        let required = add(budget, usdc)?;
        let q = settlement::fund_for(core, &market, required).await?;
        let invested = add(q.net_quote, buy_fee)?;
        let capital = add(q.limit_quote, buy_fee)?;
        let leg = Leg::Fund {
            quote_budget: q.limit_quote,
            buy_amount: q.order_amount,
            maximum_price: q.limit_price,
            required_wallet_amount: required,
            target_qty: qty,
            sell_qty: exit.sell_qty,
            minimum_target_bid: worst_b,
        };
        (q, leg, invested, capital, proceeds)
    } else {
        let received = proceeds
            .checked_sub(usdc)
            .filter(|x| *x > Decimal::ZERO)
            .ok_or("Расходы превышают выручку расчётного актива")?;
        if net.deposit_min.is_none_or(|min| received < min) {
            return Err(
                "Выручка расчётного актива не покрывает подтверждённый минимум депозита исходной биржи".into(),
            );
        }
        let q = settlement::sell_within(core, &market, received).await?;
        let final_quote = q.net_quote;
        let leg = Leg::Return {
            available_amount: received,
            minimum_price: q.limit_price,
        };
        (
            q,
            leg,
            add(principal, buy_fee)?,
            add(budget, buy_fee)?,
            final_quote,
        )
    };
    let buy_unit = invested
        .checked_div(qty)
        .ok_or("Нет цены фактических единиц A в валюты учёта")?;
    let sell_unit = final_quote
        .checked_div(exit.sell_qty)
        .ok_or("Нет цены фактических единиц B в валюты учёта")?;
    let rates = super::convert::rates(
        &core.inner.deps,
        std::slice::from_ref(cex),
        vec![market.quote.clone()],
        &d.asset.ticker,
        buy_unit,
        &other,
    )
    .await?;
    let costs = other.iter().try_fold(Decimal::ZERO, |sum, c: &Cost| {
        let value = if c.asset == d.asset.ticker {
            mul(c.amount, sell_unit)?
        } else {
            super::convert::to_usdt(c, &rates)?
        };
        add(sum, value)
    })?;
    let reserve = super::reserve::amount(
        cx.keep.ok_or("Нет горизонта перехода")?,
        final_quote,
        cx.closure_share,
        cx.units_b_per_a
            .to_buy(buy_unit)
            .map_err(|e| e.to_string())?,
        sell_unit,
    );
    let profit = final_quote - invested - costs - reserve;
    Ok(Model {
        direction: d.clone(),
        source: cex.clone(),
        wallet,
        buy: qa,
        sell: exit.qb,
        buy_limit: worst,
        sell_limit: worst_b,
        sell_qty: exit.sell_qty,
        withdrawal_qty: exit.sent_a,
        credited_target: exit.arrive,
        conversion,
        leg,
        invested_quote: invested,
        capital_quote: capital,
        proceeds_quote: final_quote,
        other_costs_quote: costs,
        reserve_quote: reserve,
        profit_quote: profit,
    })
}
