//! Файл: venues/phemex/wallet_update.rs
//! Что делает: разбор сообщений приватного канала wallet-order Phemex в изменения баланса ядра (решение 76). В
//! сообщении — не изменение, а весь баланс монеты (balanceEv), поэтому изменение = новый баланс − прежний (по прежнему
//! сообщению этой сессии); снимок (type snapshot) только запоминает балансы, событий не даёт.
//! Кто вызывает: user_stream_conn.rs, тесты.
//! От чего зависит: num.rs, core/types/balance.rs.
//! Настройки: нет.
//!
//! Официально — Spot Websocket API → Wallet-Order message (https://phemex-docs.github.io/#wallet-order-message):
//! {"wallets": [{currency, balanceEv, lockedTradingBalanceEv, lockedWithdrawEv, lastUpdateTimeNs, userID}], "orders":
//! {"open": […], "closed": […], "fills": […]}, "sequence", "timestamp" (нс), "type": "snapshot" | "incremental"}.
//! Причины изменения (депозит, вывод, сделка) и хеша транзакции в сообщении НЕТ. Поэтому вид изменения — вывод: если в
//! том же сообщении есть события ордеров — изменение от ордера бота (own); иначе рост — Deposit, уменьшение — Withdraw
//! (ядро сверяет их со своими переводами по монете и сумме, core/credit_event.rs). Приходят ли изменения баланса от
//! сделки в одном сообщении с её ордером, документация не говорит — проверить на живой записи до включения
//! venue.balance_events (вопрос Тиграну).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{Asset, BalanceChange, BalanceKind, VenueId};

use super::num;

/// Балансы монет по последнему сообщению сессии (balanceEv как пришёл). Ошибок нет.
pub type Seen = HashMap<String, i128>;

/// Разобранный кадр.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Не сообщение wallet-order.
    Other,
    /// Изменения баланса (пустые — пропущены); snapshot — это был снимок.
    Update {
        snapshot: bool,
        changes: Vec<BalanceChange>,
    },
    /// Кадр не разобран — причина.
    Bad(String),
}

/// Что делает: разбирает текстовый кадр. Принимает: текст, площадку, масштабы монет, прежние балансы (обновляются).
/// Ошибок нет: неразборчивый — Frame::Bad (прежние балансы монеты кадра не меняются).
pub fn parse(text: &str, venue: &VenueId, scales: &HashMap<String, u32>, seen: &mut Seen) -> Frame {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Frame::Other;
    };
    let Some(wallets) = v.get("wallets") else {
        return Frame::Other;
    };
    let Some(wallets) = wallets.as_array() else {
        return Frame::Bad("wallets не массив".into());
    };
    let snapshot = match v.get("type").and_then(Value::as_str) {
        Some("snapshot") => true,
        Some("incremental") => false,
        other => {
            return Frame::Bad(format!(
                "type = {other:?} (по документации snapshot или incremental)"
            ));
        }
    };
    let orders = v.get("orders").is_some_and(|o| {
        ["open", "closed", "fills"].iter().any(|k| {
            o.get(*k)
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
        })
    });
    let at = v
        .get("timestamp")
        .and_then(num::to_int)
        .and_then(|ns| i64::try_from(ns / 1_000_000).ok())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    let mut changes = Vec::new();
    for w in wallets {
        let Some(cur) = num::text_opt(w, "currency") else {
            return Frame::Bad("нет currency".into());
        };
        let Some(now) = w.get("balanceEv").and_then(num::to_int) else {
            return Frame::Bad(format!("{cur}: balanceEv не целое"));
        };
        let prev = seen.insert(cur.clone(), now);
        let (Some(prev), false) = (prev, snapshot) else {
            continue;
        };
        let Some(delta) = scales.get(&cur).and_then(|s| num::unscale(now - prev, *s)) else {
            return Frame::Bad(format!("{cur}: нет valueScale в справочнике"));
        };
        if delta.is_zero() {
            continue;
        }
        let (kind, own) = match (orders, delta.is_sign_positive()) {
            (true, _) => (BalanceKind::Other, true),
            (false, true) => (BalanceKind::Deposit, false),
            (false, false) => (BalanceKind::Withdraw, false),
        };
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
            source: format!(
                "Phemex wallet-order balanceEv{}",
                if orders { " (с ордером)" } else { "" }
            ),
            account: None,
        });
    }
    Frame::Update { snapshot, changes }
}
