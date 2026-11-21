//! Файл: venues/bitget/order_fee.rs
//! Что делает: факт торговой комиссии ордера Bitget из поля feeDetail (Get Order Info): сколько и в какой монете —
//! в получаемой монете сделки или в BGB (скидка BGB, урок 23).
//! Кто вызывает: order_state.rs (result), tests/bitget_real_data.rs.
//! От чего зависит: num.rs.
//! Настройки: нет.
//!
//! Официально — Get Order Info GET /api/v2/spot/trade/orderInfo
//! https://www.bitget.com/docs/catalog/classic-spot-trade/classic-spot-trade#get-order-info: feeDetail — СТРОКА с JSON;
//! newFees: c — «deducted by coupons, unit: currency obtained from the transaction», d — «deducted by BGB, unit: BGB»,
//! r — «remaining amount deducted from the coins obtained from the transaction» (когда BGB не хватает), t — «total
//! trading fees, unit: currency obtained from the transaction»; deduction и totalDeductionFee — «Ignore». Старый
//! формат — ключ BGB (deduction, feeCoinCode, totalDeductionFee, totalFee) — встречается только у старых ордеров; у
//! ордеров бота не ожидается: без newFees при исполненном ордере — факт не подтверждён (ошибка).
//! Знак значений (минус у списания?) в документации не указан — берётся модуль (проверить на живом ответе).
//! Решение 219: комиссия — список по монетам. BGB не хватило на всю комиссию — d в BGB и r в получаемой монете (живой
//! ордер 08.11 XTER: d = 0.0000197 BGB, r = 5.32956 XTER, t = 5.33536) — обе части, а не ошибка.

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::Fee;

use super::num;

/// Тикер BGB — монета скидки Bitget (имя монеты в API, как в описании поля d «unit: BGB»). Не настройка: имя монеты API.
pub const BGB: &str = "BGB";

/// Что делает: комиссия ордера по монетам (решение 219).
/// Принимает: значение feeDetail (строка JSON или пусто), получаемую монету сделки.
/// Возвращает: части комиссии (≥ 0): без BGB — получаемая монета (t); BGB хватило — BGB (d); BGB не хватило —
/// BGB (d) и получаемая монета (r). Пустой feeDetail — пустой список.
/// Ошибки: текст причины — feeDetail не JSON, нет newFees, значение не число, часть купоном (c ≠ 0: как он делит
/// комиссию, документация не говорит).
pub fn parse(fee_detail: Option<&Value>, received: &str) -> Result<Vec<Fee>, String> {
    let text = match fee_detail {
        None | Some(Value::Null) => return Ok(vec![]),
        Some(Value::String(s)) if s.trim().is_empty() => return Ok(vec![]),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    };
    let v: Value = serde_json::from_str(&text).map_err(|_| format!("feeDetail не JSON: {text}"))?;
    let fees = v
        .get("newFees")
        .ok_or_else(|| format!("в feeDetail нет newFees: {text}"))?;
    let get = |k: &str| {
        num::dec_opt(fees, k)
            .map(|x| x.unwrap_or(Decimal::ZERO).abs())
            .map_err(|_| format!("newFees.{k} не число: {text}"))
    };
    let (c, d, r, t) = (get("c")?, get("d")?, get("r")?, get("t")?);
    if !c.is_zero() {
        return Err(format!(
            "комиссия частично купоном (newFees.c = {c}) — как делится, в документации нет: {text}"
        ));
    }
    let mut out = vec![];
    if d.is_zero() {
        // Без BGB: вся комиссия — в получаемой монете (t; r — остаток после BGB, у ордера без скидки может быть 0).
        Fee::add(&mut out, received, t.max(r));
    } else {
        // BGB покрыл всю комиссию (r = 0) или часть — остаток r из получаемой монеты.
        Fee::add(&mut out, BGB, d);
        Fee::add(&mut out, received, r);
    }
    Ok(out)
}
