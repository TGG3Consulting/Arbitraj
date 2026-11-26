//! Файл: core/settlement.rs.
//! Что делает: рассчитывает количественные и денежные стороны лимитной конверсии.
//! Кто вызывает: расчёт маршрута, исполнение и API.
//! От чего зависит: замороженный стакан, правила рынка и комиссии.
//! Настройки: пара и площадка передаются через Market.

use super::{
    settlement_market::Market,
    types::{CostKind, Side, VenueId},
};
use crate::gateway::{
    events::SpotPairState,
    frozen::{QuoteSnapshot, deepest},
};
use rust_decimal::Decimal;
use serde::Serialize;

/// Обычный путь допускает лишь непосредственно совместимые единицы.
pub fn require_same(a: &str, b: &str) -> Result<(), String> {
    if a == b {
        Ok(())
    } else {
        Err(format!(
            "Для {a}/{b} требуется отдельная конверсия на исходной бирже"
        ))
    }
}
/// Старые возвраты требуют валюту отправителя. Необязательное поле получателя не добавляет
/// требований старой установке; явно объявленная другая валюта требует собственного пути.
pub fn direct_return(sender: &str, recipient: Option<&str>) -> Result<(), String> {
    if sender != crate::settings::evm_profiles::LEDGER_QUOTE
        || recipient.is_some_and(|v| v != sender)
    {
        return Err(
            "Для различных расчётных валют нужны dex.settlement_contract и исполнимая конверсия"
                .into(),
        );
    }
    Ok(())
}

/// Модель одной конверсии на CEX. Обе валюты именованы: размеры и комиссии сохраняются в своей валюте.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conversion {
    pub asset_symbol: String,
    pub quote_symbol: String,
    pub venue: VenueId,
    pub side: Side,
    pub order_amount: Decimal,
    pub net_asset: Decimal,
    pub net_quote: Decimal,
    pub fee_asset: Decimal,
    pub fee_quote: Decimal,
    pub limit_price: Decimal,
    /// Для покупки — необходимый резерв валюты учёта по лимитной цене; для продажи — нижняя граница выручки.
    pub limit_quote: Decimal,
}

/// Лимитный ордер IOC/FOK на бирже-источнике. Объём расчётного актива округляется вниз, цена берётся с предельного уровня.
/// Возвращает суммы сторон с комиссией. Нехватка глубины или неизвестная валюта комиссии закрывает план.
pub async fn conversion(
    snap: &QuoteSnapshot,
    rules: &SpotPairState,
    market: &Market,
    side: Side,
    quantity: Decimal,
) -> Result<Conversion, String> {
    let cex = &market.cex;
    let book = snap.book().ok_or("Нет стакана спотовой конверсии")?;
    if book.venue != *cex
        || book.asset.ticker != market.asset.ticker
        || rules.ticker != market.asset.ticker
    {
        return Err(
            "Стакан или правила пары не совпадают с исходной биржей и расчётным токеном".into(),
        );
    }
    if !rules.online
        || rules.order_block.is_some()
        || !(if side == Side::Buy {
            rules.api_buy
        } else {
            rules.api_sell
        })
    {
        return Err("Требуемая сторона пары спотовой конверсии закрыта для ордеров".into());
    }
    if rules.step_size <= Decimal::ZERO || quantity <= Decimal::ZERO {
        return Err("Для конверсии нужны положительные размер и шаг расчётного актива".into());
    }
    let qty = quantity
        - quantity
            .checked_rem(rules.step_size)
            .ok_or("Невозможно округлить количество расчётного актива")?;
    if qty <= Decimal::ZERO {
        return Err("Количество расчётного актива меньше шага пары".into());
    }
    let limit_price = deepest(book, side, qty)
        .filter(|p| *p > Decimal::ZERO)
        .ok_or("Недостаточно глубины спотовой конверсии")?;
    super::costs_quote::order_limits(&cex.0, Some(rules), qty, limit_price)
        .map_err(|e| e.to_string())?;
    let q = snap.quote(qty).await.map_err(|e| e.message)?;
    if q.venue != *cex
        || q.side != side
        || q.asset.ticker != market.asset.ticker
        || q.fillable_qty != qty
        || q.avg_price <= Decimal::ZERO
    {
        return Err("Котировка конверсии не покрывает объём на нужной стороне пары".into());
    }
    if !q.costs.iter().any(|fee| fee.kind == CostKind::TradeFee) {
        return Err("Биржа не сообщила комиссию конверсии спотовой конверсии".into());
    }
    let mut fee_asset = Decimal::ZERO;
    let mut fee_quote = Decimal::ZERO;
    for fee in q.costs.iter().filter(|c| !c.in_price) {
        if fee.paid_in.is_some() || fee.amount < Decimal::ZERO {
            return Err(
                "Для комиссии отдельной монетой требуется исполнимая оценка и резерв этой монеты"
                    .into(),
            );
        }
        match fee.asset.as_str() {
            s if s == market.asset.ticker => fee_asset += fee.amount,
            s if s == market.quote => fee_quote += fee.amount,
            ticker => {
                return Err(format!(
                    "Неизвестна стоимость комиссии конверсии в {ticker}"
                ));
            }
        }
    }
    let gross = qty
        .checked_mul(q.avg_price)
        .ok_or("Сумма конверсии вне диапазона")?;
    let (net_asset, net_quote) = if side == Side::Buy {
        (qty - fee_asset, gross + fee_quote)
    } else {
        (qty + fee_asset, gross - fee_quote)
    };
    if net_asset <= Decimal::ZERO || net_quote <= Decimal::ZERO {
        return Err("Комиссия превышает результат конверсии".into());
    }
    let limit_gross = qty
        .checked_mul(limit_price)
        .ok_or("Сумма лимитного ордера вне диапазона")?;
    let limit_fee = if fee_quote.is_zero() {
        Decimal::ZERO
    } else {
        fee_quote
            .checked_div(gross)
            .and_then(|rate| rate.checked_mul(limit_gross))
            .ok_or("Комиссия лимитного ордера вне диапазона")?
    };
    let limit_quote = if side == Side::Buy {
        limit_gross + limit_fee
    } else {
        limit_gross - limit_fee
    };
    Ok(Conversion {
        asset_symbol: market.asset.ticker.clone(),
        quote_symbol: market.quote.clone(),
        limit_quote,
        venue: cex.clone(),
        side,
        order_amount: qty,
        net_asset,
        net_quote,
        fee_asset,
        fee_quote,
        limit_price,
    })
}

pub(super) use super::settlement_funding::fund_for;
pub use super::settlement_quotes::preview;
pub(super) use super::settlement_quotes::sell_within;
