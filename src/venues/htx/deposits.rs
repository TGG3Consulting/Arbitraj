//! Файл: venues/htx/deposits.rs
//! Что делает: статус депозита HTX по хешу транзакции (последние записи монеты, сравнение tx-hash через
//! gateway::tx_hash::same) и удержания вывода (решение 150, урок 22): депозиты в состоянии confirmed — торговать можно,
//! выводить нельзя до safe.
//! Кто вызывает: mod.rs (deposit_status, withdraw_hold).
//! От чего зависит: records.rs, num.rs, ctx.rs, gateway/tx_hash.rs, core/types (WithdrawHold).
//! Настройки: нет.
//!
//! Официально (https://huobiapi.github.io/docs/spot/v1/en/): «List of possible deposit state»: unknown «on-chain transfer
//! has not been received», confirming «waits for first confirmation», confirmed «confirmed for at least one block»,
//! safe «multiple on-chain confirmations», orphan «confirmed but currently in an orphan branch». APIv2 Currency & Chains:
//! numOfFastConfirmations — «trading allowed, withdrawal disallowed», numOfConfirmations — «deposit success». Отсюда:
//! confirmed — зачислено для торговли (Credited) и удержано от вывода (как MEXC PRE_SUCCESS); safe — зачислено полностью;
//! orphan — ещё не окончательно (ждём). Как HTX ограничивает вывод при confirmed, в документации нет — правило вычета
//! решает ядро (core/precheck_withdraw.rs); адаптер отдаёт список.

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, HoldItem, Transfer, TransferStatus, WithdrawHold};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::{Ctx, ticker};
use super::{num, records};

/// Состояние «зачислено для торговли, вывод удержан» — confirmed документации.
pub const CONFIRMED: &str = "confirmed";

/// Статус депозита по хешу транзакции. Ошибки: депозит не виден HTX, неизвестное состояние, связь.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, tx: &str) -> GatewayResult<Transfer> {
    let list = records::fetch(ctx, "deposit", Some(&asset.ticker)).await?;
    let rec = list
        .iter()
        .find(|r| num::text_opt(r, "tx-hash").is_some_and(|h| tx_hash::same(&h, tx)))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::DepositNotCredited,
                None,
                format!("Депозит {tx} пока не виден HTX"),
            ))
        })?;
    let status = state(rec)?;
    records::transfer(ctx, rec, tx, status)
}

/// Состояние записи депозита → статус ядра. Ошибка BadResponse — состояние вне перечня документации.
pub fn state(rec: &Value) -> GatewayResult<TransferStatus> {
    Ok(match num::text(rec, "state")?.as_str() {
        CONFIRMED | "safe" => TransferStatus::Credited,
        "unknown" | "confirming" | "orphan" => TransferStatus::Sent,
        other => {
            return Err(super::errors::bad(
                &format!("неизвестное состояние депозита {other}"),
                &rec.to_string(),
            ));
        }
    })
}

/// Удержания вывода сейчас: депозиты в состоянии confirmed среди последних записей всех монет.
/// Ошибки: ключ, права, связь, непонятный ответ.
pub async fn withdraw_hold(ctx: &Ctx) -> GatewayResult<WithdrawHold> {
    let list = records::fetch(ctx, "deposit", None).await?;
    hold(&list)
}

/// Разбор записей депозитов в удержания: только state = confirmed. Ошибки: BadResponse — нет currency / amount.
pub fn hold(list: &[Value]) -> GatewayResult<WithdrawHold> {
    let mut items = vec![];
    for rec in list {
        if num::text_opt(rec, "state").as_deref() != Some(CONFIRMED) {
            continue;
        }
        let asset = ticker(&num::text(rec, "currency")?);
        let amount = num::dec(rec, "amount")?;
        items.push(HoldItem {
            reason: reason(rec, &asset, amount),
            asset,
            amount,
        });
    }
    Ok(WithdrawHold { items })
}

/// Текст для человека: «депозит 45 USDT подтверждён для торговли, вывод — после safe (сеть …, tx …)». Ошибок нет.
fn reason(rec: &Value, asset: &str, amount: Decimal) -> String {
    let field = |f: &str| num::text_opt(rec, f).unwrap_or_else(|| "не указано".into());
    format!(
        "депозит {} {asset} зачислен для торговли (confirmed), вывести можно после safe (сеть {}, tx {})",
        amount.normalize(),
        field("chain"),
        field("tx-hash")
    )
}
