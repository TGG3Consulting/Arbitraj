//! Файл: venues/binance/deposits.rs
//! Что делает: статус депозита Binance по хешу транзакции (параметр txId истории депозитов) и удержания вывода
//! (решение 150, урок 22): депозиты со статусом 6 «зачислено, вывод заблокирован» — торговать можно, выводить нельзя.
//! Хеши сравниваются через gateway::tx_hash::same.
//! Кто вызывает: mod.rs (deposit_status, withdraw_hold из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, transfers.rs (transfer), gateway/tx_hash.rs.
//! Настройки: нет своих.
//!
//! Официально: GET /sapi/v1/capital/deposit/hisrec (Wallet «Deposit History (supporting network)», вес IP 1) — coin,
//! status, txId, startTime/endTime (по умолчанию последние 90 дней), limit (по умолчанию 1000); status: 0 ожидает,
//! 1 успех, 2 отклонён, 6 «credited but cannot withdraw», 7 ошибочный депозит, 8 ждёт подтверждения пользователя;
//! travelRuleStatus: 1 — нужно заполнить данные депозита по travel rule; поля amount, coin, network, txId,
//! confirmTimes, unlockConfirm, walletType (0 спот, 1 фонд — по описанию walletType у вывода).
//! Как у MEXC PRE_SUCCESS (решение 107): статус 6 — ещё не Credited (ядро ждёт полного зачисления); 6 отдаётся как
//! удержание вывода. Проверить на первом живом депозите.

use reqwest::Method;
use serde_json::Value;

use crate::core::types::{Asset, HoldItem, Transfer, TransferStatus, WithdrawHold};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::transfers::transfer;
use super::{num, rest};

/// Путь истории депозитов.
const HISREC: &str = "/sapi/v1/capital/deposit/hisrec";
/// Статус 6 — «credited but cannot withdraw» (перечень документации, не настройка).
const CREDITED_LOCKED: i64 = 6;

/// Статус депозита по хешу. Ошибки: не виден (DepositNotCredited), отклонён, неописанный статус, связь.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, hash: &str) -> GatewayResult<Transfer> {
    let p = vec![("coin", asset.ticker.clone()), ("txId", hash.to_string())];
    let data = rest::signed(ctx, Method::GET, HISREC, p, Op::Other).await?;
    let rec = data.as_array().and_then(|a| {
        a.iter()
            .find(|r| num::text_opt(r, "txId").is_some_and(|t| tx_hash::same(&t, hash)))
    });
    let rec = rec.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::DepositNotCredited,
            None,
            format!("Депозит {hash} пока не виден Binance"),
        ))
    })?;
    if num::int(rec, "travelRuleStatus").ok() == Some(1) {
        let msg = format!(
            "Binance: депозит {hash} ждёт данных travel rule (travelRuleStatus 1) — заполнить в кабинете"
        );
        return Err(ctx
            .tag(GatewayError::new(ErrorCode::DepositNotCredited, None, msg))
            .with_raw(rec.to_string()));
    }
    let status = match num::int(rec, "status")? {
        1 => TransferStatus::Credited,
        0 | 6 | 8 => TransferStatus::Sent,
        s @ (2 | 7) => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::DepositNotCredited,
                    None,
                    format!(
                        "Binance: депозит {hash} не зачислен, статус {s} (2 отклонён, 7 ошибочный)"
                    ),
                )
                .with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(errors::bad(
                &format!("статус депозита {other} не описан в документации Binance"),
                &rec.to_string(),
            ));
        }
    };
    transfer(ctx, rec, hash, status)
}

/// Удержания вывода сейчас: депозиты со статусом 6 за окно API по умолчанию. Ошибки: ключ, связь, формат.
pub async fn withdraw_hold(ctx: &Ctx) -> GatewayResult<WithdrawHold> {
    let p = vec![("status", CREDITED_LOCKED.to_string())];
    let data = rest::signed(ctx, Method::GET, HISREC, p, Op::Other).await?;
    parse_hold(&data)
}

/// Разбор истории депозитов в удержания (только status 6). Ошибка BadResponse — не массив или поля не числа.
pub fn parse_hold(data: &Value) -> GatewayResult<WithdrawHold> {
    let list = data
        .as_array()
        .ok_or_else(|| errors::bad("история депозитов не массив", &data.to_string()))?;
    let mut items = vec![];
    for rec in list {
        if num::int(rec, "status")? != CREDITED_LOCKED {
            continue;
        }
        let asset = num::text(rec, "coin")?;
        let amount = num::dec(rec, "amount")?;
        let field = |f: &str| num::text_opt(rec, f).unwrap_or_else(|| "не указано".into());
        let reason = format!(
            "зачислено, вывод заблокирован: {amount} {asset} (подтверждений {}, нужно для разблокировки {}, сеть {})",
            field("confirmTimes"),
            field("unlockConfirm"),
            field("network")
        );
        items.push(HoldItem {
            reason,
            asset,
            amount,
        });
    }
    Ok(WithdrawHold { items })
}
