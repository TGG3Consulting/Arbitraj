//! Файл: core/balances_view.rs
//! Что делает: типы памяти панели «Балансы» (решения 144, 174, 176; вынесено из balances_cache.rs — размер файла):
//! ответ биржи (счета, строки, кнопка «Продать»), ответ кошелька DEX, JSON панели (как GET /api/balances/{venue}) и
//! свободные USDT торгового счёта (для перевода USDT).
//! Кто вызывает: core/balances_cache.rs, core/usdt_sources.rs, api/balances.rs, notify/ctl/*.
//! От чего зависит: rust_decimal, chrono, serde_json.
//! Настройки: нет.

use std::time::Instant;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};

/// Строка счёта: монета, свободно, заморожено, кнопка «Продать» (None — кнопки нет; Err — недоступна с причиной).
#[derive(Debug, Clone)]
pub struct RowView {
    pub asset: String,
    pub free: Decimal,
    pub locked: Decimal,
    pub sell: Option<Result<(), String>>,
}

/// Счёт биржи. trading — это торговый (спот) счёт: с него выводит и на него зачисляет перегон USDT.
#[derive(Debug, Clone)]
pub struct AccountView {
    pub account: String,
    pub trading: bool,
    pub rows: Vec<RowView>,
    pub error: Option<String>,
    pub note: Option<String>,
}

/// Ответ биржи: когда получен и счета (или причина сбоя).
#[derive(Debug, Clone)]
pub struct VenueView {
    pub venue: String,
    pub at: DateTime<Utc>,
    pub got: Instant,
    pub accounts: Vec<AccountView>,
    pub error: Option<String>,
}

/// Кошелёк DEX: BNB и USDT (тикер — venue.quote_asset площадки DEX).
#[derive(Debug, Clone)]
pub struct WalletView {
    pub wallet: String,
    pub at: DateTime<Utc>,
    pub got: Instant,
    pub bnb: Option<Decimal>,
    pub usdt: Option<Decimal>,
    pub quote: Option<String>,
    pub error: Option<String>,
}

impl VenueView {
    /// JSON панели «Балансы» (как GET /api/balances/{venue} до решения 176). Ошибок нет.
    pub fn to_json(&self) -> Value {
        let accounts: Vec<Value> = self.accounts.iter().map(|a| {
            let rows: Vec<Value> = a.rows.iter().map(|b| {
                let sell = match &b.sell {
                    None => Value::Null,
                    Some(Ok(())) => json!({ "ok": true }),
                    Some(Err(why)) => json!({ "ok": false, "why": why }),
                };
                json!({ "venue": self.venue, "asset": b.asset, "free": b.free, "locked": b.locked, "sell": sell })
            }).collect();
            json!({ "account": a.account, "balances": rows, "error": a.error, "note": a.note })
        }).collect();
        json!({ "venue": self.venue, "at": self.at, "accounts": accounts, "error": self.error })
    }

    /// Свободные USDT на торговом счёте. Принимает: тикер. Возвращает: сумму или None — торговый счёт не прочитан.
    pub fn trading_free(&self, ticker: &str) -> Option<Decimal> {
        let a = self
            .accounts
            .iter()
            .find(|a| a.trading && a.error.is_none())?;
        Some(
            a.rows
                .iter()
                .find(|r| r.asset == ticker)
                .map_or(Decimal::ZERO, |r| r.free),
        )
    }
}
