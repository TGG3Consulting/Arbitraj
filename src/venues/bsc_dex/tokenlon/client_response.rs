//! Проверка суммы, сторон рынка, получателя и срока ответа издателя PMMV5.
use super::{Failure, Query};
use crate::core::types::Side;
use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::Value;
use std::str::FromStr;

fn available(response: &Value) -> Result<(), Failure> {
    match response["exchangeable"].as_bool() {
        Some(true) => Ok(()),
        Some(false) if response["reason"] == "USER_BALANCE_INSUFFICIENT" => Err(Failure::Balance),
        Some(false) => Err(Failure::NoOffer),
        None => Err(Failure::BadMessage),
    }
}
fn decimal(v: &Value) -> Option<Decimal> {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    Decimal::from_str(&s)
        .or_else(|_| Decimal::from_scientific(&s))
        .ok()
}
pub(super) fn rate_value(response: &Value, amount: Decimal) -> Result<Decimal, Failure> {
    available(response)?;
    let min = decimal(&response["minAmount"]).ok_or(Failure::BadMessage)?;
    let max = decimal(&response["maxAmount"]).ok_or(Failure::BadMessage)?;
    if amount < min || amount > max {
        return Err(Failure::NoOffer);
    }
    decimal(&response["rate"])
        .filter(|v| *v > Decimal::ZERO)
        .ok_or(Failure::BadMessage)
}
pub(super) fn pmm_order(
    response: Value,
    owner: Address,
    q: &Query,
    now: u64,
    signed: bool,
) -> Result<Value, Failure> {
    available(&response)?;
    let mut response = response;
    let order = response["order"].take();
    let address = |k: &str| order[k].as_str()?.parse::<Address>().ok();
    let raw = |k: &str| order[k].as_str()?.parse::<U256>().ok();
    let specified = match q.side {
        Side::Sell => raw("takerAssetAmount"),
        Side::Buy => raw("makerAssetAmount"),
    };
    if specified.is_none() || specified != q.base_units() {
        return Err(Failure::NoOffer);
    }
    let expiry = order["expirationTimeSeconds"]
        .as_u64()
        .or_else(|| order["expirationTimeSeconds"].as_str()?.parse().ok());
    if order["protocol"] != "PMMV5"
        || address("feeRecipientAddress") != Some(owner)
        || address("takerAssetAddress") != Some(q.input)
        || address("makerAssetAddress") != Some(q.output)
        || raw("takerAssetAmount").is_none_or(|v| v.is_zero())
        || raw("makerAssetAmount").is_none_or(|v| v.is_zero())
        || expiry.is_none_or(|t| t <= now)
    {
        return Err(Failure::NoOffer);
    }
    if signed
        && !order["makerWalletSignature"]
            .as_str()
            .and_then(|v| v.strip_prefix("0x"))
            .is_some_and(|v| {
                !v.is_empty()
                    && v.len().is_multiple_of(2)
                    && v.bytes().all(|b| b.is_ascii_hexdigit())
            })
    {
        return Err(Failure::NoOffer);
    }
    Ok(order)
}
