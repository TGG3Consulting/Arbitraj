//! Файл: venues/poloniex/activity.rs
//! Что делает: общее у истории депозитов и выводов Poloniex — окно запроса (start / end обязательны), запрос записей
//! одного вида и перевод записи в формат ядра (Transfer).
//! Кто вызывает: deposits.rs, transfers.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs, core/types.
//! Настройки: venue.history_window_s [poloniex] — на сколько секунд назад от «сейчас» искать запись (не задана — поиск
//! депозита и вывода не работает, вывод не отправляется: искать его после отправки было бы нечем — урок 7).
//!
//! Официально — Wallets Activity Records GET /wallets/activity (https://api-docs.poloniex.com/spot/api/private/wallet;
//! «10» на VIP0 — Spot → API → Rate Limits): start*, end* — «UNIX timestamp» (в секундах — по SDK
//! polosdk/spot/rest/wallets.py: get_activity «in seconds»), activityType «deposits» / «withdrawals». Ответ — объект
//! {deposits: [...], withdrawals: [...]}. Поиска по id или хешу на стороне биржи нет, постраничного чтения нет, предел
//! промежутка start…end не указан. Депозит: depositNumber, currency, address, amount («excludes network fees»),
//! confirmations, txid, timestamp, status, depositType («0» — из сети). Вывод: withdrawalRequestsId, currency, address,
//! amount («including the fee»), fee, timestamp, status, txid, withdrawOrderId, withdrawalType.

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Вид записей: имя параметра activityType и имя массива ответа (одно и то же по документации).
pub const DEPOSITS: &str = "deposits";
pub const WITHDRAWALS: &str = "withdrawals";

/// Окно поиска: (start, end) в секундах — venue.history_window_s назад от сейчас. Ошибка NotConfigured — не задана.
pub fn window(ctx: &Ctx) -> GatewayResult<(i64, i64)> {
    let s = ctx.int("venue.history_window_s")?;
    let end = Utc::now().timestamp();
    let span = i64::try_from(s).unwrap_or(i64::MAX);
    Ok((end.saturating_sub(span), end))
}

/// Что делает: записи одного вида в окне. Принимает: вид (DEPOSITS / WITHDRAWALS).
/// Возвращает: массив записей. Ошибки: NotConfigured (окно), ключи, связь, коды Poloniex, ответ не того вида.
pub async fn fetch(ctx: &Ctx, kind: &'static str) -> GatewayResult<Vec<Value>> {
    let (start, end) = window(ctx)?;
    let q = vec![
        ("start", start.to_string()),
        ("end", end.to_string()),
        ("activityType", kind.to_string()),
    ];
    let data = rest::signed(ctx, Method::GET, "/wallets/activity", q, None, Op::Other).await?;
    list(&data, kind)
}

/// Массив записей вида kind из ответа. Нет массива или null — пусто. Ошибка BadResponse — не массив.
pub fn list(data: &Value, kind: &str) -> GatewayResult<Vec<Value>> {
    match data.get(kind) {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(other) => Err(super::errors::bad(
            &format!("wallets/activity: {kind} не массив"),
            &other.to_string(),
        )),
    }
}

/// Перевод формата ядра из записи. Тикер — запрошенный ядром (в записи currency может быть именем монеты в сети —
/// проверить на живом ответе); комиссия — fee (у вывода). Ошибки: BadResponse — нет amount.
pub fn transfer(
    ctx: &Ctx,
    rec: &Value,
    ticker: &str,
    network: &str,
    venue_ref: &str,
    status: TransferStatus,
) -> GatewayResult<Transfer> {
    let fee = num::dec_opt(rec, "fee")?
        .map(|f| Cost::new(CostKind::WithdrawFee, f.abs(), ticker.to_string(), false));
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker: ticker.to_string(),
            bsc_contract: None,
        },
        network: network.to_string(),
        amount: num::dec(rec, "amount")?,
        status,
        venue_ref: venue_ref.to_string(),
        tx_hash: num::text_opt(rec, "txid"),
        fee,
        updated_at: Utc::now(),
    })
}
