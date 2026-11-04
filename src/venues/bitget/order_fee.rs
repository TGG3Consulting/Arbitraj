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

use rust_decimal::Decimal;
use serde_json::Value;

use super::num;

/// Тикер BGB — монета скидки Bitget (имя монеты в API, как в описании поля d «unit: BGB»). Не настройка: имя монеты API.
pub const BGB: &str = "BGB";

/// Что делает: комиссия ордера одной монетой.
/// Принимает: значение feeDetail (строка JSON или пусто), получаемую монету сделки.
/// Возвращает: (комиссия ≥ 0, монета комиссии); пустой feeDetail — (0, получаемая монета).
/// Ошибки: текст причины — feeDetail не JSON, нет newFees, значение не число, комиссия сразу в двух монетах (BGB и
/// монета сделки) или купоном (c ≠ 0: как он делит комиссию, документация не говорит).
pub fn parse(fee_detail: Option<&Value>, received: &str) -> Result<(Decimal, String), String> {
    let text = match fee_detail {
        None | Some(Value::Null) => return Ok((Decimal::ZERO, received.to_string())),
        Some(Value::String(s)) if s.trim().is_empty() => {
            return Ok((Decimal::ZERO, received.to_string()));
        }
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
    match (d.is_zero(), r.is_zero()) {
        // Без BGB: вся комиссия — в получаемой монете (t; r — остаток после BGB, у ордера без скидки может быть 0).
        (true, _) => Ok((t.max(r), received.to_string())),
        (false, true) => Ok((d, BGB.to_string())),
        (false, false) => Err(format!(
            "комиссия в двух монетах: BGB {d} и {received} {r} — ядро принимает одну монету комиссии: {text}"
        )),
    }
}
