//! Файл: venues/bitget/records.rs
//! Что делает: общее у истории депозитов и выводов Bitget — окно запроса (startTime / endTime обязательны), запрос
//! записей и перевод записи в формат ядра (Transfer).
//! Кто вызывает: deposits.rs, transfers.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs, core/types.
//! Настройки: venue.history_window_s [bitget] — на сколько секунд назад от «сейчас» искать запись (не задана — поиск
//! депозита и вывода не работает, вывод не отправляется: искать его после отправки было бы нечем — урок 7).
//!
//! Официально (https://www.bitget.com/docs/catalog/classic-spot-account/classic-spot-account): Get Deposit Records
//! GET /api/v2/spot/wallet/deposit-records и Get Withdrawal Records GET /api/v2/spot/wallet/withdrawal-records —
//! startTime*, endTime* (мс), coin, orderId (у выводов ещё clientOid — «when passed, other parameters are ignored»),
//! idLessThan, limit («default 20, max 100»); «10 times/1s». Наибольший промежуток startTime…endTime не указан.
//! Поля записи: orderId, tradeId («TX ID. when dest is on_chain, it's the on chain hash value»), coin, size, fee (у
//! вывода), status, chain, dest (on_chain / internal_transfer), cTime, uTime.

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::rest::Params;
use super::{num, rest};

/// Наибольший limit истории — официально «max 100».
pub const LIMIT_MAX: &str = "100";

/// Окно поиска: (startTime, endTime) в мс — venue.history_window_s назад от сейчас. Ошибка NotConfigured — не задана.
pub fn window(ctx: &Ctx) -> GatewayResult<(i64, i64)> {
    let s = ctx.int("venue.history_window_s")?;
    let end = Utc::now().timestamp_millis();
    let span = i64::try_from(s.saturating_mul(1000)).unwrap_or(i64::MAX);
    Ok((end.saturating_sub(span), end))
}

/// Что делает: записи истории в окне. Принимает: путь (deposit-records / withdrawal-records), доп. параметры.
/// Возвращает: массив записей. Ошибки: NotConfigured (окно), ключи, связь, коды Bitget, data не массив.
pub async fn fetch(ctx: &Ctx, path: &str, mut q: Params) -> GatewayResult<Vec<Value>> {
    let (start, end) = window(ctx)?;
    q.push(("startTime", start.to_string()));
    q.push(("endTime", end.to_string()));
    let data = rest::signed(ctx, Method::GET, path, q, None, Op::Other).await?;
    match data {
        Value::Array(a) => Ok(a),
        Value::Null => Ok(vec![]),
        other => Err(super::errors::bad(
            &format!("{path}: data не массив"),
            &other.to_string(),
        )),
    }
}

/// Перевод формата ядра из записи (вывод или депозит). Хеш — tradeId только при dest = on_chain (у внутреннего
/// перевода tradeId — не хеш сети). Ошибки: BadResponse — нет coin или size.
pub fn transfer(
    ctx: &Ctx,
    rec: &Value,
    venue_ref: &str,
    status: TransferStatus,
) -> GatewayResult<Transfer> {
    let ticker = num::text(rec, "coin")?.to_ascii_uppercase();
    let fee = num::dec_opt(rec, "fee")?
        .map(|f| Cost::new(CostKind::WithdrawFee, f.abs(), ticker.clone(), false));
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker,
            bsc_contract: None,
        },
        network: num::text_opt(rec, "chain").unwrap_or_default(),
        amount: num::dec(rec, "size")?,
        status,
        venue_ref: venue_ref.to_string(),
        tx_hash: hash(rec),
        fee,
        updated_at: Utc::now(),
    })
}

/// Хеш транзакции записи: tradeId при dest = on_chain (или без dest). Ошибок нет.
pub fn hash(rec: &Value) -> Option<String> {
    let dest = num::text_opt(rec, "dest");
    if dest.as_deref().is_some_and(|d| d != "on_chain") {
        return None;
    }
    num::text_opt(rec, "tradeId")
}
