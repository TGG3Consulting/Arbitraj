//! Файл: core/settlement_cash.rs.
//! Что делает: вычисляет фактические дебет и кредит, ставит либо сверяет IOC.
//! Кто вызывает: исполнитель поручения.
//! От чего зависит: результат биржи, комиссия и баланс.
//! Настройки: выключатели, резервы, dry.enabled.
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

/// Для Buy дебет/остаток — валюты учёта, кредит — расчётного актива. Для Sell дебет/остаток — расчётного актива, кредит — валюты учёта.
pub fn cash(
    m: &super::settlement_market::Market,
    fill: &SpotFill,
    budget: Decimal,
) -> Result<CashFill, String> {
    let trade = &fill.trade;
    let req = &trade.request;
    if !m.matches(&req.asset)
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
            s if s == m.asset.ticker => {
                usdc = usdc
                    .checked_add(f.amount)
                    .ok_or("Комиссия расчётного актива вне диапазона")?
            }
            s if s == m.quote => {
                usdt = usdt
                    .checked_add(f.amount)
                    .ok_or("Комиссия валюты учёта вне диапазона")?
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
/// quantity — желаемое количество расчётного актива; budget — максимум списания в валюте стороны.
pub async fn convert(
    core: &Core,
    deal: &str,
    m: &super::settlement_market::Market,
    side: Side,
    quantity: Decimal,
    budget: Decimal,
    price_bound: Decimal,
) -> Result<CashFill, String> {
    let cex = &m.cex;
    let deps = &core.inner.deps;
    let gw = deps.gw.get_as(cex, None).await.map_err(|e| e.message)?;
    if !gw.supports_settlement_order()
        || deal.is_empty()
        || budget <= Decimal::ZERO
        || price_bound <= Decimal::ZERO
    {
        return Err("Нет условий исполнимого денежного шага конверсии".into());
    }
    let req = match effect::previous_order(&deps.db, m, deal, cex, side)
        .await
        .map_err(|e| e.message)?
    {
        Some(req) => req,
        None => {
            super::executor::buy_gate(core)?;
            if !super::registry_sync::enabled(core, cex) {
                return Err("Исходная биржа выключена".into());
            }
            let row = m.network(core, side == Side::Buy)?;
            let plan = if side == Side::Sell {
                settlement::sell_within(core, m, quantity.min(budget)).await?
            } else {
                settlement::preview(core, m, side, quantity).await?
            };
            let (ticker, required) = if side == Side::Buy {
                if plan.limit_price > price_bound {
                    return Err("Цена покупки расчётного актива превысила допустимый лимит".into());
                }
                (m.quote.as_str(), plan.limit_quote)
            } else {
                if plan.limit_price < price_bound {
                    return Err("Цена продажи расчётного актива ниже допустимого лимита".into());
                }
                (m.asset.ticker.as_str(), plan.net_asset)
            };
            let free = super::limits::free_now(core, cex, None, ticker).await?;
            let free = if side == Side::Buy {
                super::usdt_hold::spendable(
                    free,
                    &super::usdt_hold::held_except(core, cex, None, Some(deal)),
                )
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
                qty: plan.order_amount,
                worst_price: plan.limit_price,
                deal_id: deal.into(),
                route: None,
                exit_position: side == Side::Sell,
                remeasure: None,
            }
        }
    };
    m.ensure(core)?;
    let check = m.guard(core, &m.asset, side == Side::Buy);
    let spot = gw.clone();
    let pair = m.clone();
    let fill = crate::gateway::settlement_intent::checked(
        move |key| {
            check(key)?;
            if !spot.matches_settlement_order(key, &pair.asset.ticker, &pair.quote) {
                return Err(crate::gateway::errors::GatewayError::new(
                    crate::gateway::errors::ErrorCode::BadResponse,
                    Some(pair.cex.clone()),
                    "Подготовленный ордер относится к другой торговой паре",
                ));
            }
            Ok(())
        },
        effect::order(&deps.db, m, gw.as_ref(), req),
    )
    .await
    .map_err(|e| e.message)?;
    cash(m, &fill, budget)
}
