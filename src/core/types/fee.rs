//! Файл: core/types/fee.rs
//! Что делает: часть фактической комиссии ордера — сколько и в какой монете (решение 219); у монеты без рынка —
//! стоимость в USDT по курсу из официальной документации площадки (Fee.usd: баллы HTX, решение 232 дополнение).
//! Вынесена из core/types/trade.rs (файл вырос больше 200 строк); путь types::trade::Fee сохранён переэкспортом.
//! Кто вызывает: core/types/trade.rs (TradeResult.fees), адаптеры (разбор факта ордера), core/fills.rs (учёт).
//! От чего зависит: rust_decimal, serde.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Одна часть фактической комиссии ордера: сколько и в какой монете (решение 219).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fee {
    /// В чём взята: тикер монеты сделки, котируемая монета (USDT), монета скидки биржи (BGB, GT, MX, BNB, KCS),
    /// BNB за газ у DEX.
    pub asset: String,
    /// Сумма в единицах `asset`, неотрицательная.
    pub amount: Decimal,
    /// Стоимость части в USDT по курсу, который задаёт ОФИЦИАЛЬНАЯ документация площадки, когда у монеты комиссии нет
    /// рынка (HTX: баллы HB / Point Card — «1 pts = 1 USDT fee», venues/htx/order_fee.rs). Ядро берёт её в затраты
    /// как есть (core/fills.rs). None — пересчёт по живой цене (core/convert.rs), как раньше. Старые записи — None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<Decimal>,
}

impl Fee {
    /// Что делает: список из одной части комиссии (у площадки, что сообщает одну монету). Ноль — пустой список.
    /// Принимает: сумму, монету. Возвращает: Vec<Fee>. Ошибок нет.
    pub fn single(amount: Decimal, asset: impl Into<String>) -> Vec<Fee> {
        if amount.is_zero() {
            return vec![];
        }
        vec![Fee {
            asset: asset.into(),
            amount,
            usd: None,
        }]
    }

    /// Что делает: добавляет часть комиссии, суммируя по монете (несколько сделок ордера с разными монетами).
    /// Нулевая сумма не добавляется (пустая монета при ненулевой сумме — добавляется: факт виден, пересчёт в USDT
    /// не пройдёт и это пишется в хронологию). Принимает: список, монету, сумму. Ошибок нет.
    pub fn add(list: &mut Vec<Fee>, asset: &str, amount: Decimal) {
        if amount.is_zero() {
            return;
        }
        match list.iter_mut().find(|f| f.asset == asset) {
            Some(f) => f.amount += amount,
            None => list.push(Fee {
                asset: asset.to_string(),
                amount,
                usd: None,
            }),
        }
    }

    /// Что делает: добавляет часть комиссии с документированной стоимостью в USDT (у монеты комиссии нет рынка —
    /// баллы HTX), суммируя по монете и стоимости. Нулевая сумма не добавляется. Принимает: список, монету, сумму,
    /// стоимость в USDT. Ошибок нет.
    pub fn add_usd(list: &mut Vec<Fee>, asset: &str, amount: Decimal, usd: Decimal) {
        if amount.is_zero() {
            return;
        }
        match list.iter_mut().find(|f| f.asset == asset) {
            Some(f) => {
                f.amount += amount;
                f.usd = Some(f.usd.unwrap_or_default() + usd);
            }
            None => list.push(Fee {
                asset: asset.to_string(),
                amount,
                usd: Some(usd),
            }),
        }
    }

    /// Что делает: текст «0.1 USDT + 0.00002 BGB»; часть с документированной стоимостью — «0.05 HBPOINT (= 0.05 USDT)»;
    /// пусто — «0». Принимает: список. Ошибок нет.
    pub fn text(list: &[Fee]) -> String {
        if list.is_empty() {
            return "0".into();
        }
        list.iter()
            .map(|f| match f.usd {
                Some(u) => format!("{} {} (= {u} USDT)", f.amount, f.asset),
                None => format!("{} {}", f.amount, f.asset),
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }
}
