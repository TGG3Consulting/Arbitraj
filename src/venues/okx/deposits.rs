//! Файл: venues/okx/deposits.rs
//! Что делает: статус депозита OKX по хешу транзакции и удержания вывода (решение 150): депозиты в состоянии 1
//! «Deposit credited» — зачислены (торговать можно), но вывести их нельзя до minWdUnlockConfirm подтверждений.
//! Кто вызывает: mod.rs (deposit_status, withdraw_hold).
//! От чего зависит: rest.rs, num.rs, networks_parse.rs (сеть ядра), ctx.rs, gateway/tx_hash.rs, core/types.
//! Настройки: нет своих.
//!
//! Официально — «Funding Account → REST API → Get deposit history» GET /api/v5/asset/deposit-history (6 запросов/с,
//! User ID) https://www.okx.com/docs-v5/en/: параметры ccy, txId, state, limit («The maximum is 100; The default is 100»);
//! строки — ccy, chain, amt, txId, state, actualDepBlkConfirm, ts, depId, to (адрес). state: 0 «waiting for
//! confirmation», 1 «deposit credited», 2 «deposit successful», 8 «pending due to temporary deposit suspension on this
//! crypto currency», 11 «match the address blacklist», 12 «account or deposit is frozen», 13 «sub-account deposit
//! interception», 14 «KYC limit». Справочник монет: minDepArrivalConfirm — зачисление, minWdUnlockConfirm — «required
//! for withdrawal of a deposit»: состояние 1 — зачислено, вывод ещё держится (как MEXC PRE_SUCCESS, решение 150).

use chrono::Utc;
use serde_json::Value;

use crate::core::types::{Asset, HoldItem, Transfer, TransferStatus, WithdrawHold};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::Ctx;
use super::networks_parse::core_network;
use super::{num, rest};

/// Путь истории депозитов.
const PATH: &str = "/api/v5/asset/deposit-history";
/// Состояние «Deposit credited» — значение документации OKX (перечисление state), не настройка.
pub const CREDITED_HELD: &str = "1";

/// Статус депозита по хешу транзакции. Ошибки: депозит не виден OKX, не зачислен (11–14), неизвестное состояние.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, tx: &str) -> GatewayResult<Transfer> {
    let q = vec![("ccy", asset.ticker.clone()), ("txId", tx.to_string())];
    let data = rest::get(ctx, PATH, q).await?;
    let rec = data.as_array().and_then(|a| {
        a.iter()
            .find(|r| num::text_opt(r, "txId").is_some_and(|t| tx_hash::same(&t, tx)))
    });
    let rec = rec.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::DepositNotCredited,
            None,
            format!("Депозит {tx} пока не виден OKX"),
        ))
    })?;
    let status = state(rec).map_err(|e| ctx.tag(e))?;
    transfer(ctx, rec, status)
}

/// Состояние записи → статус ядра. 1 и 2 — зачислено; 0, 8 — ждём; 11–14 — не зачислен (с причиной).
/// Ошибки: DepositNotCredited (11–14), BadResponse (неизвестное состояние).
pub fn state(rec: &Value) -> GatewayResult<TransferStatus> {
    let s = num::text(rec, "state")?;
    let why = match s.as_str() {
        "1" | "2" => return Ok(TransferStatus::Credited),
        "0" | "8" => return Ok(TransferStatus::Sent),
        "11" => "адрес в чёрном списке (11)",
        "12" => "счёт или депозит заморожен (12)",
        "13" => "перехват депозита субсчёта (13)",
        "14" => "ограничение KYC (14)",
        other => {
            return Err(super::errors::bad(
                &format!("неизвестное состояние депозита {other}"),
                &rec.to_string(),
            ));
        }
    };
    Err(GatewayError::new(
        ErrorCode::DepositNotCredited,
        None,
        format!("OKX: депозит не зачислен — {why}"),
    )
    .with_raw(rec.to_string()))
}

/// Перевод формата ядра из записи депозита. Ошибки: нет ccy или amt.
fn transfer(ctx: &Ctx, rec: &Value, status: TransferStatus) -> GatewayResult<Transfer> {
    let ticker = num::text(rec, "ccy")?;
    let chain = num::text_opt(rec, "chain").unwrap_or_default();
    Ok(Transfer {
        venue: ctx.venue(),
        network: core_network(&ticker, &chain),
        asset: Asset {
            ticker,
            bsc_contract: None,
        },
        amount: num::dec(rec, "amt")?,
        status,
        venue_ref: num::text_opt(rec, "depId").unwrap_or_default(),
        tx_hash: num::text_opt(rec, "txId"),
        fee: None,
        updated_at: Utc::now(),
    })
}

/// Удержания вывода сейчас: депозиты в состоянии 1 (последние 100 записей — предел limit).
/// Ошибки: ключ, права, связь, лимит частоты, непонятный ответ (BadResponse).
pub async fn withdraw_hold(ctx: &Ctx) -> GatewayResult<WithdrawHold> {
    let q = vec![("state", CREDITED_HELD.to_string())];
    let data = rest::get(ctx, PATH, q).await?;
    hold(&data)
}

/// Разбор истории депозитов в удержания: только записи со state = 1. Ошибка BadResponse — не массив или нет полей.
pub fn hold(data: &Value) -> GatewayResult<WithdrawHold> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("история депозитов не массив", &data.to_string()))?;
    let mut items = vec![];
    for rec in list
        .iter()
        .filter(|r| num::text_opt(r, "state").as_deref() == Some(CREDITED_HELD))
    {
        let asset = num::text(rec, "ccy")?;
        let amount = num::dec(rec, "amt")?;
        let field = |f: &str| num::text_opt(rec, f).unwrap_or_else(|| "не указано".into());
        let reason = format!(
            "зачислено, вывод ещё закрыт: {amount} {asset} (state 1, подтверждений {}, сеть {}; для вывода нужно \
             minWdUnlockConfirm справочника)",
            field("actualDepBlkConfirm"),
            field("chain")
        );
        items.push(HoldItem {
            asset,
            amount,
            reason,
        });
    }
    Ok(WithdrawHold::of(items))
}
