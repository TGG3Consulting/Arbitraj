//! Файл: venues/mexc/account_update.rs
//! Что делает: разбор двоичного кадра потока аккаунта MEXC (обёртка PushDataV3ApiWrapper, тело PrivateAccountV3Api,
//! канал spot@private.account.v3.api.pb) в изменение баланса ядра.
//! По официальной документации (https://www.mexc.com/api-docs/spot-v3/websocket-user-data-streams/spot-account-update):
//! vcoinName — монета, balanceAmountChange — изменение доступного, frozenAmountChange — изменение замороженного,
//! type — причина (WITHDRAW, WITHDRAW_FEE, DEPOSIT, DEPOSIT_FEE, ENTRUST, ENTRUST_PLACE, ENTRUST_CANCEL, TRADE_FEE,
//! ENTRUST_UNFROZEN, SUGAR, ETF_INDEX и др.), time — время расчёта, мс. Хеша транзакции нет.
//! Изменение баланса = balanceAmountChange + frozenAmountChange (общий баланс: постановка ордера переносит доступное в
//! замороженное — сумма 0, такие кадры пропускаются).
//! Кто вызывает: user_stream_conn.rs (тест — после первой живой записи кадра, docs/TASKS.md).
//! От чего зависит: proto.rs, core/types/balance.rs, prost.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use prost::Message as _;
use rust_decimal::Decimal;

use crate::core::types::{Asset, BalanceChange, BalanceKind, VenueId};

use super::proto::{Body, Wrapper};

/// Разобранный кадр.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Не изменение баланса (другой канал, пустое изменение).
    Other,
    /// Изменение баланса: причина type и изменение (None — сумма изменений 0).
    Update {
        reason: String,
        change: Option<BalanceChange>,
    },
    /// Кадр не разобран — причина.
    Bad(String),
}

/// Вид изменения по причине type. Возвращает: (вид, своё ли). Ошибок нет.
pub fn kind(reason: &str) -> (BalanceKind, bool) {
    match reason {
        "DEPOSIT" => (BalanceKind::Deposit, false),
        "WITHDRAW" => (BalanceKind::Withdraw, false),
        // Ордера на споте делает бот (аккаунт только его — решение 76).
        "ENTRUST" | "ENTRUST_PLACE" | "ENTRUST_CANCEL" | "TRADE_FEE" | "ENTRUST_UNFROZEN" => {
            (BalanceKind::Other, true)
        }
        _ => (BalanceKind::Other, false),
    }
}

/// Разбирает двоичный кадр. Принимает: байты, площадку. Возвращает: Frame. Ошибок нет: неразборчивый — Frame::Bad.
pub fn parse(data: &[u8], venue: &VenueId) -> Frame {
    let w = match Wrapper::decode(data) {
        Ok(w) => w,
        Err(e) => return Frame::Bad(format!("protobuf: {e}")),
    };
    let Some(Body::PrivateAccount(a)) = w.body else {
        return Frame::Other;
    };
    let num = |s: &str, f: &str| -> Result<Decimal, String> {
        if s.is_empty() {
            return Ok(Decimal::ZERO);
        }
        s.parse::<Decimal>()
            .map_err(|_| format!("{}: {f} не число ({s})", a.vcoin_name))
    };
    let delta = match (
        num(&a.balance_amount_change, "balanceAmountChange"),
        num(&a.frozen_amount_change, "frozenAmountChange"),
    ) {
        (Ok(b), Ok(f)) => b + f,
        (Err(e), _) | (_, Err(e)) => return Frame::Bad(e),
    };
    if a.vcoin_name.is_empty() {
        return Frame::Bad("нет монеты vcoinName".into());
    }
    let at = DateTime::<Utc>::from_timestamp_millis(a.time)
        .or_else(|| {
            w.create_time
                .and_then(DateTime::<Utc>::from_timestamp_millis)
        })
        .unwrap_or_else(Utc::now);
    let reason = a.r#type.clone();
    if delta.is_zero() {
        return Frame::Update {
            reason,
            change: None,
        };
    }
    let (kind, own) = kind(&reason);
    Frame::Update {
        change: Some(BalanceChange {
            venue: venue.clone(),
            asset: Asset {
                ticker: a.vcoin_name.clone(),
                bsc_contract: None,
            },
            delta,
            kind,
            own,
            tx_hash: None,
            at,
            source: format!("MEXC private.account type={reason}"),
        }),
        reason,
    }
}
