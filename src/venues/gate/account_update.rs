//! Файл: venues/gate/account_update.rs
//! Что делает: подписка на приватный канал spot.balances Gate (с подписью) и разбор его уведомлений в изменения
//! баланса ядра (BalanceChange).
//! Кто вызывает: user_stream.rs, tests/gate_real_data.rs.
//! От чего зависит: ctx.rs (ключи), core/types/balance.rs, hmac, sha2.
//! Настройки: нет.
//!
//! Официально (Spot WebSocket v4 https://www.gate.com/docs/developers/apiv4/ws/en/ ; SDK gatews
//! https://github.com/gate/gatews, python/gate_ws/client.py):
//! - подпись приватного канала: auth {method: "api_key", KEY, SIGN}, SIGN = hex(HMAC-SHA512(секрет,
//!   «channel=<канал>&event=<событие>&time=<time>»)); time — секунды, расхождение не больше 60 с;
//! - spot.balances (event update), result — массив: timestamp, timestamp_ms (строки), user, currency, change
//!   («Changed amount»), total («Total spot balance»), available, freeze, freeze_change, change_type: withdraw, deposit,
//!   trade-fee-deduct, order-create, order-match, order-update, margin-transfer, future-transfer,
//!   cross-margin-transfer, referral-fee, sub-transfer, spot-transfer, other. Хеша транзакции нет.
//!
//! Изменение общего баланса = change (поле рядом с total; изменение доступного и замороженного по отдельности в
//! документации не разведено — проверить на живой записи, вопрос Тиграну). change = 0 — пропуск.

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::Sha512;

use crate::core::types::{Asset, BalanceChange, BalanceKind, VenueId};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;

/// Канал балансов спота (официальное имя).
pub const CHANNEL: &str = "spot.balances";

/// Запрос подписки с подписью. Ошибки: ключи (SecretsLocked, NotConfigured).
pub fn subscribe(ctx: &Ctx) -> GatewayResult<String> {
    let (key, secret) = ctx.keys()?;
    let time = Utc::now().timestamp();
    let text = format!("channel={CHANNEL}&event=subscribe&time={time}");
    let mut mac = Hmac::<Sha512>::new_from_slice(secret.as_bytes()).map_err(|_| {
        ctx.tag(crate::gateway::errors::GatewayError::new(
            crate::gateway::errors::ErrorCode::InvalidCredentials,
            None,
            "Пустой секретный ключ Gate",
        ))
    })?;
    mac.update(text.as_bytes());
    let sign = hex::encode(mac.finalize().into_bytes());
    let auth = json!({ "method": "api_key", "KEY": key.as_str(), "SIGN": sign });
    Ok(json!({ "time": time, "channel": CHANNEL, "event": "subscribe", "auth": auth }).to_string())
}

/// Вид изменения по change_type. Возвращает: (вид, своё ли). Ошибок нет.
pub fn kind(t: &str) -> (BalanceKind, bool) {
    match t {
        "deposit" => (BalanceKind::Deposit, false),
        "withdraw" => (BalanceKind::Withdraw, false),
        // Ордера на споте делает бот (аккаунт только его — решение 76).
        "order-create" | "order-match" | "order-update" | "trade-fee-deduct" => {
            (BalanceKind::Other, true)
        }
        _ => (BalanceKind::Other, false),
    }
}

/// Разобранный кадр.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Не уведомление балансов.
    Other,
    /// Изменения баланса (пустые — пропущены) и причины (change_type) каждой записи.
    Update {
        reasons: Vec<String>,
        changes: Vec<BalanceChange>,
    },
    /// Кадр не разобран — причина.
    Bad(String),
}

/// Разбирает текстовый кадр. Принимает: текст, площадку. Ошибок нет: неразборчивый — Frame::Bad.
pub fn parse(text: &str, venue: &VenueId) -> Frame {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Frame::Other;
    };
    if v.get("channel").and_then(Value::as_str) != Some(CHANNEL)
        || v.get("event").and_then(Value::as_str) != Some("update")
    {
        return Frame::Other;
    }
    let Some(items) = v.get("result").and_then(Value::as_array) else {
        return Frame::Bad("result не массив".into());
    };
    let (mut reasons, mut changes) = (Vec::new(), Vec::new());
    for i in items {
        let s = |f: &str| {
            i.get(f)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let (cur, reason) = (s("currency"), s("change_type"));
        if cur.is_empty() {
            return Frame::Bad("нет currency".into());
        }
        let Ok(delta) = s("change").parse::<Decimal>() else {
            return Frame::Bad(format!("{cur}: change не число ({})", s("change")));
        };
        reasons.push(reason.clone());
        if delta.is_zero() {
            continue;
        }
        let at = s("timestamp_ms")
            .split('.')
            .next()
            .and_then(|ms| ms.parse::<i64>().ok())
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .unwrap_or_else(Utc::now);
        let (kind, own) = kind(&reason);
        changes.push(BalanceChange {
            venue: venue.clone(),
            asset: Asset {
                ticker: cur,
                bsc_contract: None,
            },
            delta,
            kind,
            own,
            tx_hash: None,
            at,
            source: format!("Gate spot.balances change_type={reason}"),
            account: None,
        });
    }
    Frame::Update { reasons, changes }
}
