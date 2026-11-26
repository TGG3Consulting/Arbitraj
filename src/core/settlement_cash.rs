//! Денежная часть IOC-конверсии на исходной спотовой бирже.
//! Расчёт и итог различены: точное заполнение и списания берутся из ответа исполненного ордера.
use super::{settlement, settlement_effect as effect};
use crate::core::{
    engine::Core,
    types::{Side, TradeRequest, VenueId},
};
use crate::gateway::settlement_intent::SpotFill;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Нетто обеих сторон одного подтверждённого ордера и неиспользованная сумма исходного актива.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CashFill {
    pub venue: VenueId,
    pub order_id: String,
    pub side: Side,
    pub debit: Decimal,
    pub credit: Decimal,
    pub unspent: Decimal,
}

/// Для Buy дебет/остаток — USDT, кредит — USDC. Для Sell дебет/остаток — USDC, кредит — USDT.
pub fn cash(fill: &SpotFill, budget: Decimal) -> Result<CashFill, String> {
    let trade = &fill.trade;
    let req = &trade.request;
    if !effect::base_usdc(&req.asset)
        || trade.venue_ref.is_empty()
        || trade.chain.is_some()
        || trade.filled_qty > req.qty
        || trade.filled_qty < Decimal::ZERO
        || fill.gross_quote < Decimal::ZERO
        || budget < Decimal::ZERO
        || trade.filled_qty.is_zero() != fill.gross_quote.is_zero()
    {
        return Err("Некорректный идентификатор или количество фактической конверсии".into());
    }
    let mut usdt = Decimal::ZERO;
    let mut usdc = Decimal::ZERO;
    for f in &trade.fees {
        if f.amount < Decimal::ZERO {
            return Err("Некорректная комиссия исполнения".into());
        }
        match f.asset.as_str() {
            "USDC" => {
                usdc = usdc
                    .checked_add(f.amount)
                    .ok_or("Комиссия USDC вне диапазона")?
            }
            "USDT" => {
                usdt = usdt
                    .checked_add(f.amount)
                    .ok_or("Комиссия USDT вне диапазона")?
            }
            _ if f.amount.is_zero() => {}
            _ => {
                return Err(format!(
                    "Требуется оценка фактически списанной комиссии {} {}",
                    f.amount, f.asset
                ));
            }
        }
    }
    let (debit, credit) = match req.side {
        Side::Buy => (
            fill.gross_quote.checked_add(usdt),
            trade.filled_qty.checked_sub(usdc),
        ),
        Side::Sell => (
            trade.filled_qty.checked_add(usdc),
            fill.gross_quote.checked_sub(usdt),
        ),
    };
    let debit = debit.ok_or("Дебет конверсии вне диапазона")?;
    let credit = credit.ok_or("Кредит конверсии вне диапазона")?;
    let unspent = budget
        .checked_sub(debit)
        .ok_or("Остаток конверсии вне диапазона")?;
    if credit < Decimal::ZERO || unspent < Decimal::ZERO {
        return Err("Фактическая комиссия или дебет превысили средства этого этапа".into());
    }
    Ok(CashFill {
        venue: req.venue.clone(),
        order_id: trade.venue_ref.clone(),
        side: req.side,
        debit,
        credit,
        unspent,
    })
}

/// Первая отправка проверяет текущий стакан, счёт, лимит и политику торгов.
/// После перезапуска сверяется только ранее отправленный запрос, даже если книга или баланс изменились.
/// quantity — желаемое количество USDC; budget — максимум списания в валюте стороны.
pub async fn convert(
    core: &Core,
    deal: &str,
    cex: &VenueId,
    side: Side,
    quantity: Decimal,
    budget: Decimal,
    price_bound: Decimal,
) -> Result<CashFill, String> {
    let deps = &core.inner.deps;
    let gw = deps.gw.get_as(cex, None).await.map_err(|e| e.message)?;
    if !gw.supports_settlement_order()
        || deal.is_empty()
        || budget <= Decimal::ZERO
        || price_bound <= Decimal::ZERO
    {
        return Err("Нет условий исполнимого денежного шага Base".into());
    }
    let req = match effect::previous_order(&deps.db, deal, cex, side)
        .await
        .map_err(|e| e.message)?
    {
        Some(req) => req,
        None => {
            super::executor::buy_gate(core)?;
            if !super::registry_sync::enabled(core, cex) {
                return Err("Исходная биржа выключена".into());
            }
            let row = settlement::usdc_network(core, cex, side == Side::Buy)?;
            let plan = if side == Side::Sell {
                settlement::sell_within(core, cex, quantity.min(budget)).await?
            } else {
                settlement::preview(core, cex, side, quantity).await?
            };
            let (ticker, required) = if side == Side::Buy {
                if plan.limit_price > price_bound {
                    return Err("Цена покупки USDC превысила допустимый лимит".into());
                }
                ("USDT", plan.limit_usdt)
            } else {
                if plan.limit_price < price_bound {
                    return Err("Цена продажи USDC ниже допустимого лимита".into());
                }
                ("USDC", plan.net_usdc)
            };
            let free = super::limits::free_now(core, cex, None, ticker).await?;
            let free = if side == Side::Buy {
                super::usdt_hold::spendable(free, &super::usdt_hold::held(core, cex, None))
            } else {
                free
            };
            if required > budget || required > free {
                return Err(format!(
                    "Для конверсии требуется {required} {ticker}, у этапа {budget}, свободно {free}"
                ));
            }
            TradeRequest {
                venue: cex.clone(),
                asset: row.asset,
                side,
                qty: plan.order_usdc,
                worst_price: plan.limit_price,
                deal_id: deal.into(),
                route: None,
                exit_position: side == Side::Sell,
                remeasure: None,
            }
        }
    };
    let fill = effect::order(&deps.db, gw.as_ref(), req)
        .await
        .map_err(|e| e.message)?;
    cash(&fill, budget)
}
