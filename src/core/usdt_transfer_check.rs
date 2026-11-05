//! Файл: core/usdt_transfer_check.rs
//! Что делает: решение 176 — типы ручного перевода USDT и проверка суммы (вынесено из usdt_transfer_plan.rs — размер
//! файла): конец перевода (биржа или кошелёк DEX), сводка «сколько уйдёт / комиссия / дойдёт» и её JSON, пределы суммы и
//! чистая проверка: больше нуля, не больше «можно вывести», не меньше минимума вывода, дойдёт не меньше минимума депозита.
//! Кто вызывает: core/usdt_transfer_plan.rs (quote), core/usdt_transfer.rs, core/usdt_sources.rs, api/usdt_transfer.rs,
//! notify/ctl/*.
//! От чего зависит: core/types (VenueId, DexAccountId), rust_decimal.
//! Настройки: нет.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{DexAccountId, VenueId};

/// Конец перевода: площадка и (у DEX) кошелёк.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct End {
    pub venue: VenueId,
    pub wallet: Option<DexAccountId>,
}

impl End {
    /// Ключ конца: «биржа» или «dex/кошелёк» — метка «идёт перевод» и экраны. Ошибок нет.
    pub fn key(&self) -> String {
        match &self.wallet {
            Some(w) => format!("{}/{}", self.venue.0, w.0),
            None => self.venue.0.clone(),
        }
    }
}

/// Сводка перевода: сколько уйдёт, комиссия, сколько дойдёт.
#[derive(Debug, Clone)]
pub struct Quote {
    pub from: End,
    pub to: End,
    pub ticker: String,
    /// Название сети BSC у отправителя.
    pub network: String,
    /// Свободно USDT на отправителе сейчас.
    pub available: Decimal,
    /// Можно вывести (свободно минус удержания биржи, решение 166).
    pub withdrawable: Decimal,
    /// Сумма вывода (вниз до точности вывода биржи).
    pub amount: Decimal,
    /// Комиссия вывода в USDT; None — отправитель кошелёк DEX (платит газ в BNB).
    pub fee: Option<Decimal>,
    /// Наименьшая сумма вывода (минимум вывода отправителя; минимум депозита получателя — на «дойдёт»).
    pub min: Decimal,
    pub arrives: Decimal,
    /// Пометка: удержания биржи или «биржа не сообщает удержаний».
    pub note: Option<String>,
}

impl Quote {
    /// JSON для интерфейса. Ошибок нет.
    pub fn to_json(&self) -> Value {
        json!({ "from": self.from.key(), "to": self.to.key(), "ticker": self.ticker, "network": self.network,
            "available": self.available, "withdrawable": self.withdrawable, "amount": self.amount, "fee": self.fee,
            "min": self.min, "arrives": self.arrives, "note": self.note })
    }
}

/// Пределы суммы перевода: можно вывести, минимум вывода, комиссия вывода, минимум депозита получателя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub withdrawable: Decimal,
    pub min: Decimal,
    pub fee: Decimal,
    pub deposit_min: Decimal,
}

/// Проверка суммы (чистая функция): больше нуля, не больше «можно вывести», не меньше минимума вывода, дойдёт больше
/// нуля и не меньше минимума депозита. Принимает: сумму, пределы, тикер, (откуда, куда) для текста.
/// Возвращает: сколько дойдёт. Ошибки: текст причины.
pub fn check_amount(
    amount: Decimal,
    l: &Limits,
    t: &str,
    (from, to): (&str, &str),
) -> Result<Decimal, String> {
    let arrives = amount - l.fee;
    if amount <= Decimal::ZERO {
        return Err(format!(
            "сумма — ноль: можно вывести {} {t}",
            l.withdrawable.round_dp(4)
        ));
    }
    if amount > l.withdrawable {
        return Err(format!(
            "{amount} {t} больше, чем можно вывести: {} {t}",
            l.withdrawable.round_dp(4)
        ));
    }
    if amount < l.min {
        return Err(format!(
            "{amount} {t} меньше минимума вывода {from}: {} {t}",
            l.min
        ));
    }
    if arrives <= Decimal::ZERO || arrives < l.deposit_min {
        return Err(format!(
            "дойдёт {} {t} — меньше минимума депозита {to}: {} {t}",
            arrives.round_dp(4),
            l.deposit_min
        ));
    }
    Ok(arrives)
}
