//! Файл: venues/poloniex/account_update.rs
//! Что делает: сообщения приватного WebSocket Poloniex (вход auth с подписью, подписка balances) и разбор его кадров;
//! перевод уведомлений balances (полные остатки монеты) в изменения баланса ядра (BalanceChange) — по разнице с
//! прошлым остатком монеты (опорные остатки — REST при входе и прошлые уведомления, по version).
//! Кто вызывает: user_stream.rs, tests/poloniex_real_data.rs.
//! От чего зависит: ctx.rs (ключи), sign.rs, core/types/balance.rs.
//! Настройки: нет.
//!
//! Официально:
//! - Authentication (https://api-docs.poloniex.com/spot/websocket/authentication): {"event": "subscribe", "channel":
//!   ["auth"], "params": {key, signTimestamp, signature}}; ответ {"channel": "auth", "data": {"success": true|false,
//!   "message"?, "ts"}}; при сбое ключа — «re-authenticate».
//! - Balance (https://api-docs.poloniex.com/spot/websocket/balance): подписка {"event": "subscribe", "channel":
//!   ["balances"]}; уведомление {"channel": "balances", "data": [{changeTime, accountId, accountType, eventType,
//!   available, currency, id, userId, hold, version, ts}]}; eventType: place_order, canceled_order, match_order,
//!   transfer_in, transfer_out, deposit, withdraw; version — «client determines whether the current data is the latest».
//!   Поля «изменение» в уведомлении НЕТ — только остатки available и hold: изменение = (available + hold) − прошлый
//!   остаток (проверить на живой записи). Хеша транзакции нет.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{Asset, Balance, BalanceChange, BalanceKind, VenueId};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::sign;

/// Канал балансов (официальное имя).
pub const CHANNEL: &str = "balances";

/// Сообщение входа с подписью. Ошибки: ключи (SecretsLocked, NotConfigured), пустой секрет.
pub fn auth(ctx: &Ctx) -> GatewayResult<String> {
    let keys = ctx.keys()?;
    let ts = Utc::now().timestamp_millis();
    let signature = sign::hmac_b64(sign::ws_payload(ts).as_bytes(), keys.secret.as_bytes())
        .map_err(|e| ctx.tag(e))?;
    let params = json!({ "key": keys.key.as_str(), "signTimestamp": ts, "signature": signature });
    Ok(json!({ "event": "subscribe", "channel": ["auth"], "params": params }).to_string())
}

/// Сообщение подписки на балансы. Ошибок нет.
pub fn subscribe() -> String {
    json!({ "event": "subscribe", "channel": [CHANNEL] }).to_string()
}

/// Остаток монеты из уведомления.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub currency: String,
    /// available + hold.
    pub total: Decimal,
    pub event_type: String,
    pub version: Option<i64>,
    pub at: DateTime<Utc>,
}

/// Разобранный кадр.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Ответ на вход: успех или текст отказа.
    Auth(Result<(), String>),
    /// Ошибка сервера (event error) — текст как пришёл.
    Error(String),
    /// Остатки монет.
    Balances(Vec<Row>),
    /// Не уведомление балансов (pong, подтверждение подписки…).
    Other,
    /// Кадр не разобран — причина.
    Bad(String),
}

/// Разбирает текстовый кадр. Ошибок нет: неразборчивый — Frame::Bad.
pub fn parse(text: &str) -> Frame {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Frame::Bad("кадр не JSON".into());
    };
    if v.get("event").and_then(Value::as_str) == Some("error") {
        return Frame::Error(text.to_string());
    }
    match v.get("channel").and_then(Value::as_str) {
        Some("auth") if v.get("event").is_none() => {
            let d = v.get("data");
            let ok = d.and_then(|d| d.get("success")).and_then(Value::as_bool) == Some(true);
            if ok {
                return Frame::Auth(Ok(()));
            }
            let m = d
                .and_then(|d| d.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("success не true");
            Frame::Auth(Err(m.to_string()))
        }
        Some(CHANNEL) if v.get("event").is_none() => rows(&v),
        _ => Frame::Other,
    }
}

/// Записи data уведомления balances. Ошибок нет: неразборчивое — Frame::Bad.
fn rows(v: &Value) -> Frame {
    let Some(items) = v.get("data").and_then(Value::as_array) else {
        return Frame::Bad("balances: нет массива data".into());
    };
    let mut out = Vec::with_capacity(items.len());
    for i in items {
        let s = |f: &str| i.get(f).and_then(Value::as_str).unwrap_or_default();
        let num = |f: &str| s(f).trim().parse::<Decimal>().ok();
        let cur = s("currency").to_ascii_uppercase();
        let (Some(avail), Some(hold)) = (num("available"), num("hold")) else {
            return Frame::Bad(format!("{cur}: available или hold не число"));
        };
        if cur.is_empty() {
            return Frame::Bad("запись без currency".into());
        }
        let at = i
            .get("changeTime")
            .and_then(Value::as_i64)
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .unwrap_or_else(Utc::now);
        out.push(Row {
            currency: cur,
            total: avail + hold,
            event_type: s("eventType").to_string(),
            version: i.get("version").and_then(Value::as_i64),
            at,
        });
    }
    Frame::Balances(out)
}

/// Вид изменения по eventType. Возвращает: (вид, своё ли). Ошибок нет.
pub fn kind(t: &str) -> (BalanceKind, bool) {
    match t {
        "deposit" => (BalanceKind::Deposit, false),
        "withdraw" => (BalanceKind::Withdraw, false),
        // Ордера на споте делает бот (аккаунт только его — решение 76).
        "place_order" | "canceled_order" | "match_order" => (BalanceKind::Other, true),
        _ => (BalanceKind::Other, false),
    }
}

/// Последние известные остатки монет (available + hold) и version их уведомления.
#[derive(Default)]
pub struct Totals {
    map: HashMap<String, (Decimal, Option<i64>)>,
}

impl Totals {
    /// Опорные остатки из REST (балансы спота при входе). Ошибок нет.
    pub fn seed(&mut self, rows: &[Balance]) {
        for b in rows {
            self.map
                .insert(b.asset.to_ascii_uppercase(), (b.free + b.locked, None));
        }
    }

    /// Что делает: изменение баланса по уведомлению. Монеты не было в опоре — прошлый остаток 0 (у монеты нет баланса).
    /// Уведомление со старой или той же version — пропуск. Изменение 0 — пропуск (сдвиг available ↔ hold).
    /// Возвращает: изменение ядра или None. Ошибок нет.
    pub fn apply(&mut self, venue: &VenueId, r: &Row) -> Option<BalanceChange> {
        let (old, ver) = self
            .map
            .get(&r.currency)
            .copied()
            .unwrap_or((Decimal::ZERO, None));
        if let (Some(new), Some(was)) = (r.version, ver)
            && new <= was
        {
            return None;
        }
        self.map.insert(r.currency.clone(), (r.total, r.version));
        let delta = r.total - old;
        if delta.is_zero() {
            return None;
        }
        let (kind, own) = kind(&r.event_type);
        Some(BalanceChange {
            venue: venue.clone(),
            asset: Asset {
                ticker: r.currency.clone(),
                bsc_contract: None,
            },
            delta,
            kind,
            own,
            tx_hash: None,
            at: r.at,
            source: format!("Poloniex balances eventType={}", r.event_type),
            account: None,
        })
    }
}
