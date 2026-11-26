//! Исполнимый объём основной сделки и отдельная книга USDC/USDT исходной биржи.
use super::{
    costs_source::{Live, Source},
    settlement::{self, Conversion},
    settlement_path::Leg,
};
use crate::core::{
    engine::Core,
    types::{Cost, CostKind, DexAccountId, Direction, Quote, Side, VenueId},
};
use rust_decimal::Decimal;
use serde::Serialize;

/// Стоимость/прибыль моделируются в USDT. Оценки USDC, лимиты ордеров и объёмы токена явно разделены.
#[derive(Serialize)]
pub struct Model {
    pub direction: Direction,
    pub source: VenueId,
    pub wallet: DexAccountId,
    pub buy: Quote,
    pub sell: Quote,
    pub buy_limit: Decimal,
    pub sell_limit: Decimal,
    pub sell_qty: Decimal,
    pub withdrawal_qty: Decimal,
    pub credited_target: Decimal,
    pub conversion: Conversion,
    pub leg: Leg,
    pub invested_usdt: Decimal,
    pub capital_usdt: Decimal,
    pub proceeds_usdt: Decimal,
    pub other_costs_usdt: Decimal,
    pub reserve_usdt: Decimal,
    pub profit_usdt: Decimal,
}
fn mul(a: Decimal, b: Decimal) -> Result<Decimal, String> {
    a.checked_mul(b)
        .ok_or_else(|| "Сумма вне диапазона Decimal".into())
}
fn add(a: Decimal, b: Decimal) -> Result<Decimal, String> {
    a.checked_add(b)
        .ok_or_else(|| "Сумма вне диапазона Decimal".into())
}

/// Строит обе стороны на текущих данных. Учитывает встречные заявки других сделок через Live,
/// проверенные пределы и удержания перевода токена через общий расчёт quantities.
pub async fn evaluate(core: &Core, d: &Direction, qty: Decimal) -> Result<Model, String> {
    let dex = VenueId("base_dex".into());
    let funding = d.buy_on == dex;
    if qty <= Decimal::ZERO
        || !(if funding {
            d.sell_on != dex
        } else {
            d.sell_on == dex
        })
        || !d.asset.chain.as_ref().is_some_and(|c| c.as_str() == "base")
        || d.asset.contract.is_none()
        || d.asset.contract == Some(settlement::BASE_USDC)
    {
        return Err("Нужна отличная от расчётного USDC монета Base и одна спотовая биржа".into());
    }
    let cex = if funding { &d.sell_on } else { &d.buy_on };
    let wallet = super::dex_account::of_direction_now(core, d)
        .ok_or("Нет привязанного кошелька этого направления Base")?;
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
    let cx = super::dirctx::DirCtx::base_usdc(core, d)?;
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
    let net = settlement::usdc_network(core, cex, funding)?;
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
        if cost.asset == "USDC" {
            usdc = add(usdc, cost.amount)?;
        } else if cost.asset == "USDT" && stage == "A" {
            buy_fee = add(buy_fee, cost.amount)?;
        } else {
            other.push(cost);
        }
    }
    let (conversion, leg, invested, capital, final_usdt) = if funding {
        let required = add(budget, usdc)?;
        let q = settlement::fund_for(core, cex, required).await?;
        let invested = add(q.net_usdt, buy_fee)?;
        let capital = add(q.limit_usdt, buy_fee)?;
        let leg = Leg::Fund {
            budget_usdt: q.limit_usdt,
            buy_usdc: q.order_usdc,
            maximum_price: q.limit_price,
            required_wallet_usdc: required,
            target_qty: qty,
            sell_qty: exit.sell_qty,
            minimum_target_bid: worst_b,
        };
        (q, leg, invested, capital, proceeds)
    } else {
        let received = proceeds
            .checked_sub(usdc)
            .filter(|x| *x > Decimal::ZERO)
            .ok_or("Расходы превышают выручку USDC")?;
        if net.deposit_min.is_none_or(|min| received < min) {
            return Err(
                "Выручка USDC не покрывает подтверждённый минимум депозита исходной биржи".into(),
            );
        }
        let q = settlement::sell_within(core, cex, received).await?;
        let final_usdt = q.net_usdt;
        let leg = Leg::Return {
            available_usdc: received,
            minimum_price: q.limit_price,
        };
        (
            q,
            leg,
            add(principal, buy_fee)?,
            add(budget, buy_fee)?,
            final_usdt,
        )
    };
    let buy_unit = invested
        .checked_div(qty)
        .ok_or("Нет цены фактических единиц A в USDT")?;
    let sell_unit = final_usdt
        .checked_div(exit.sell_qty)
        .ok_or("Нет цены фактических единиц B в USDT")?;
    let rates = super::convert::rates(
        &core.inner.deps,
        std::slice::from_ref(cex),
        vec!["USDT".into()],
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
        final_usdt,
        cx.closure_share,
        cx.units_b_per_a
            .to_buy(buy_unit)
            .map_err(|e| e.to_string())?,
        sell_unit,
    );
    let profit = final_usdt - invested - costs - reserve;
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
        invested_usdt: invested,
        capital_usdt: capital,
        proceeds_usdt: final_usdt,
        other_costs_usdt: costs,
        reserve_usdt: reserve,
        profit_usdt: profit,
    })
}
