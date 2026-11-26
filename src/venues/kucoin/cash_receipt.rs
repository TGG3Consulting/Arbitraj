//! Файл: venues/kucoin/cash_receipt.rs.
//! Назначение: связывает один расшифрованный KuCoin WS-снимок available с одним полным списанием.
//! Потребители: сверка именованной брони по исходному торговому accountId и venue orderId.
//! Источник: https://www.kucoin.com/docs-new/3470075w0 — /account/balance, relationContext.
//! Настройки: отсутствуют. Доказательство относится только к свободному балансу в этом же сообщении.
use super::{ctx::ID, num};
use crate::storage::cex_funds::{Balance, Booked, Bucket};
use rust_decimal::Decimal;
use serde_json::Value;

pub struct Receipt {
    pub event: String,
    pub trade: String,
    pub symbol: String,
    pub balance: Balance,
}

/// Разбирает data приватного WS-кадра. У hold/reservation суммарное изменение доступного
/// и замороженного средств равно нулю. Каждый из нескольких fills должен подтверждаться отдельно.
pub fn single_fill(
    data: &Value,
    bucket: &Bucket,
    symbol: &str,
    order: &str,
    terminal_debit: Decimal,
    epoch: i64,
) -> Option<Receipt> {
    if bucket.venue != ID
        || order.is_empty()
        || symbol.is_empty()
        || terminal_debit <= Decimal::ZERO
    {
        return None;
    }
    let text = |v: &Value, k: &str| num::text_opt(v, k).filter(|id| !id.trim().is_empty());
    let event = text(data, "relationEvent")?;
    if !matches!(event.as_str(), "trade.setted" | "trade_hf.setted")
        || text(data, "accountId")? != bucket.account
        || text(data, "currency")? != bucket.currency
    {
        return None;
    }
    let context = data.get("relationContext")?;
    if text(context, "symbol")? != symbol || text(context, "orderId")? != order {
        return None;
    }
    let read = |key| num::dec(data, key).ok();
    let free = read("available")?;
    let hold = read("hold")?;
    let total = read("total")?;
    let free_change = read("availableChange")?;
    let hold_change = read("holdChange")?;
    if free < Decimal::ZERO
        || hold < Decimal::ZERO
        || free.checked_add(hold)? != total
        || free.checked_sub(free_change)? < Decimal::ZERO
        || hold.checked_sub(hold_change)? < Decimal::ZERO
        || free_change.checked_add(hold_change)? != -terminal_debit
    {
        return None;
    }
    Some(Receipt {
        event: text(data, "relationEventId")?,
        trade: text(context, "tradeId")?,
        symbol: symbol.into(),
        balance: Balance {
            bucket: bucket.clone(),
            epoch,
            free,
            booked: vec![Booked {
                bucket: bucket.clone(),
                order: order.into(),
                debit: terminal_debit,
            }],
        },
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_a_settlement_of_the_original_trade_can_explain_this_available_balance() {
        // Синтетический terminal IOC, схема KuCoin Balance. 80 удержано; 30 оборота + 2 комиссии, 48 возвращены.
        let mut data = json!({"accountId":"native-1","currency":"USDT","available":"68","hold":"0","total":"68",
            "availableChange":"48","holdChange":"-80","relationEvent":"trade_hf.setted","relationEventId":"501",
            "relationContext":{"symbol":"X-USDT","orderId":"order-1","tradeId":"70"}});
        let b = Bucket {
            venue: ID.into(),
            account: "native-1".into(),
            currency: "USDT".into(),
        };
        let read =
            |d: &Value, amount| single_fill(d, &b, "X-USDT", "order-1", Decimal::from(amount), 3);
        let hit = read(&data, 32i64).unwrap();
        assert_eq!((hit.event.as_str(), hit.trade.as_str()), ("501", "70"));
        assert_eq!(hit.balance.free, Decimal::from(68));
        assert!(read(&data, 30).is_none());
        for (field, wrong) in [
            ("relationEvent", "trade_hf.hold"),
            ("accountId", "other"),
            ("currency", "USDC"),
            ("hold", "1"),
        ] {
            let mut other = data.clone();
            other[field] = json!(wrong);
            assert!(read(&other, 32).is_none());
        }
        data["relationContext"]["orderId"] = json!("other");
        assert!(read(&data, 32).is_none());
    }
}
