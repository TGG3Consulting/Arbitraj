//! Файл: venues/phemex/deposits.rs
//! Что делает: статус депозита Phemex по хешу транзакции: последние записи депозитов монеты (history.rs), сравнение
//! txHash с хешем (gateway::tx_hash::same).
//! Кто вызывает: mod.rs (deposit_status).
//! От чего зависит: history.rs, num.rs, symbols.rs, ctx.rs, gateway/tx_hash.rs.
//! Настройки: через history.rs — venue.history_rows [phemex].
//!
//! Официально — Query deposit history records GET /phemex-deposit/wallets/api/depositHist (https://phemex-docs.github.io/
//! #query-deposit-history-records); Deposit status (#deposit-status): Success «Succeed deposit to Phemex account»,
//! Rejected, SecurityChecking, SecurityCheckFailed, AmlCsApporve, Confirmed «Deposit confirmed», New. Зачислено — только
//! Success; Confirmed — ещё нет (подтверждён в сети, проверки не пройдены). Статуса «зачислено, но выводить нельзя» (как
//! MEXC PRE_SUCCESS, решение 150) в документации нет — удержания вывода (withdraw_hold) у Phemex нет, пока живой депозит
//! не покажет обратное. Счёт зачисления — поле depositTo (SPOT / Contract) в таблице, в примере его нет (есть accountType:
//! 1) — показывается в журнале как пришло, перевод на спот — fund_move.rs по настройке.

use crate::core::types::{Asset, Transfer, TransferStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::Ctx;
use super::history;
use super::num;
use super::symbols::Symbols;

/// Путь истории депозитов.
const PATH: &str = "/phemex-deposit/wallets/api/depositHist";

/// Статус депозита по хешу транзакции. Ошибки: депозит не виден Phemex, отклонён, неизвестный статус.
pub async fn deposit_status(
    ctx: &Ctx,
    symbols: &Symbols,
    asset: &Asset,
    tx: &str,
) -> GatewayResult<Transfer> {
    symbols.ensure_fresh(ctx).await?;
    let list = history::fetch(ctx, PATH, vec![("currency", asset.ticker.clone())]).await?;
    let rec = list
        .iter()
        .find(|r| num::text_opt(r, "txHash").is_some_and(|t| tx_hash::same(&t, tx)))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::DepositNotCredited,
                None,
                format!("Депозит {tx} пока не виден Phemex"),
            ))
        })?;
    let status = match num::text(rec, "status")?.as_str() {
        "Success" => TransferStatus::Credited,
        "New" | "Confirmed" | "SecurityChecking" | "AmlCsApporve" => TransferStatus::Sent,
        s @ ("Rejected" | "SecurityCheckFailed") => {
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::DepositNotCredited,
                    None,
                    format!("Phemex: депозит {tx} не зачислен ({s})"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(super::errors::bad(
                &format!("неизвестный статус депозита {other}"),
                &rec.to_string(),
            ));
        }
    };
    history::transfer(ctx, symbols, rec, tx, status)
}
