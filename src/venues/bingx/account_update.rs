//! Файл: venues/bingx/account_update.rs
//! Что делает: разбор кадра потока аккаунта BingX «ACCOUNT_UPDATE» (решение 76) в изменения баланса ядра.
//! По официальной документации (docs-v3, Spot → Websocket Account Data → «Subscription account balance push»):
//! кадр {e:"ACCOUNT_UPDATE", E: время мс, a:{m: причина, B:[{a: актив, bc: изменение, cw, wb, lk: заморожено}]}};
//! причины m: INIT, DEPOSIT, WITHDRAW, ORDER, FUNDING_FEE, WITHDRAW_REJECT, ADJUSTMENT, INSURANCE_CLEAR,
//! ADMIN_DEPOSIT, ADMIN_WITHDRAW, MARGIN_TRANSFER, MARGIN_TYPE_CHANGE, ASSET_TRANSFER, OPTIONS_PREMIUM_FEE,
//! OPTIONS_SETTLE_PROFIT, AUTO_EXCHANGE. Хеша транзакции в кадре нет.
//! https://bingx-api.github.io/docs-v3/#/en/Spot/Websocket%20Account%20Data/Subscription%20account%20balance%20push
//!
//! Открыто до записи живого потока (DECISIONS 76): (а) cw и wb описаны одинаково («total after change») — не
//! используются, изменение берётся из bc; (б) пример ACCOUNT_UPDATE дан без обёртки {code, dataType, data}, а пример
//! spot.executionReport — с обёрткой: принимаются обе формы, какая пришла — пишется в журнал; (в) примера с
//! m = DEPOSIT нет — приходит ли кадр в момент зачисления, неизвестно. Поэтому событие BingX само по себе ожидание не
//! закрывает — это решает настройка transfer.credit_by_event [bingx] после проверки на записи.
//!
//! Кто вызывает: venues/bingx/user_stream.rs; тесты.
//! От чего зависит: core/types/balance.rs, serde_json.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, BalanceChange, BalanceKind, VenueId};

/// Разобранный кадр.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Не ACCOUNT_UPDATE (ответ на подписку, другое событие).
    Other,
    /// ACCOUNT_UPDATE: форма кадра («без обёртки» / «в обёртке data»), причина m, изменения (INIT — пустой список:
    /// это снимок, а не изменение).
    Update {
        wrapped: bool,
        reason: String,
        changes: Vec<BalanceChange>,
    },
    /// ACCOUNT_UPDATE не разобран — причина.
    Bad(String),
}

/// Вид изменения по причине m. Возвращает: (вид, своё ли). INIT — None (снимок). Ошибок нет.
pub fn kind(reason: &str) -> Option<(BalanceKind, bool)> {
    match reason {
        "INIT" => None,
        "DEPOSIT" => Some((BalanceKind::Deposit, false)),
        "WITHDRAW" => Some((BalanceKind::Withdraw, false)),
        // Ордера на споте делает бот (аккаунт только его — решение 76).
        "ORDER" => Some((BalanceKind::Other, true)),
        _ => Some((BalanceKind::Other, false)),
    }
}

/// Разбирает текст кадра. Принимает: текст, площадку. Возвращает: Frame. Ошибок нет: неразборчивый — Frame::Bad.
pub fn parse(text: &str, venue: &VenueId) -> Frame {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Frame::Other;
    };
    let (body, wrapped) = match v.get("data") {
        Some(d) if v.get("dataType").and_then(Value::as_str) == Some("ACCOUNT_UPDATE") => (d, true),
        _ => (&v, false),
    };
    if body.get("e").and_then(Value::as_str) != Some("ACCOUNT_UPDATE") {
        return Frame::Other;
    }
    match update(body, venue) {
        Ok((reason, changes)) => Frame::Update {
            wrapped,
            reason,
            changes,
        },
        Err(why) => Frame::Bad(why),
    }
}

/// Тело ACCOUNT_UPDATE → (причина, изменения). Ошибки: текст — нет поля или число не разобрано.
fn update(body: &Value, venue: &VenueId) -> Result<(String, Vec<BalanceChange>), String> {
    let at = body
        .get("E")
        .and_then(Value::as_i64)
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .ok_or("нет времени события E")?;
    let a = body.get("a").ok_or("нет поля a")?;
    let reason = a
        .get("m")
        .and_then(Value::as_str)
        .ok_or("нет причины a.m")?
        .to_string();
    let list = a
        .get("B")
        .and_then(Value::as_array)
        .ok_or("нет списка a.B")?;
    let Some((kind, own)) = kind(&reason) else {
        return Ok((reason, Vec::new()));
    };
    let mut out = Vec::with_capacity(list.len());
    for b in list {
        let ticker = b
            .get("a")
            .and_then(Value::as_str)
            .ok_or("нет актива a.B[].a")?;
        let delta =
            number(b.get("bc")).ok_or_else(|| format!("{ticker}: изменение bc не разобрано"))?;
        if delta.is_zero() {
            continue;
        }
        out.push(BalanceChange {
            venue: venue.clone(),
            asset: Asset {
                ticker: ticker.to_string(),
                bsc_contract: None,
            },
            delta,
            kind,
            own,
            tx_hash: None,
            at,
            source: format!("ACCOUNT_UPDATE m={reason}"),
        });
    }
    Ok((reason, out))
}

/// Число из строки (как в документации: "0.00") или из числа JSON. None — не число.
fn number(v: Option<&Value>) -> Option<Decimal> {
    match v? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}
