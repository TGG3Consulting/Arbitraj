//! Файл: venues/gate/deposits.rs
//! Что делает: статус депозита Gate по хешу транзакции; хеши сравниваются через gateway::tx_hash::same (нижний
//! регистр, без 0x, до «:» — как MEXC, решение 118).
//! Кто вызывает: mod.rs (deposit_status из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, transfers.rs (transfer), gateway/tx_hash.rs.
//! Настройки: нет своих.
//!
//! Официальная документация: API v4 → Wallet → «Retrieve deposit records» GET /wallet/deposits
//! https://www.gate.com/docs/developers/apiv4/en/ (SDK: docs/WalletApi.md#list_deposits, модель DepositRecord) —
//! currency, limit («limited to 500 transactions»), без from — последние 7 дней; status: BLOCKED, DEP_CREDITED («Deposit
//! Credited, Withdrawal Pending Unlock»), DONE («Funds Credited to Spot Account»), INVALID, MANUAL, PEND, REVIEW, TRACK
//! («Tracking Block Confirmations, Pending Spot Account Credit»); txid, chain, amount.
//! Депозит зачисляется на спот («Funds Credited to Spot Account») — перевод между счетами не нужен (mod.rs,
//! deposit_to_trading). «Зачислено» = DONE. DEP_CREDITED — пока «ждём»: доступность для продажи до DONE в документации
//! не сказана (вопрос Тиграну, docs/TASKS.md). BLOCKED, INVALID — не зачислен.

use reqwest::Method;

use crate::core::types::{Asset, Transfer, TransferStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::transfers::transfer;
use super::{num, rest};

/// Сколько записей просить — наибольшее по документации («limited to 500 transactions»).
const DEPOSITS_LIMIT: &str = "500";

/// Статус депозита по хешу транзакции. Ошибки: депозит не найден (ещё не виден Gate), не зачислен, неизвестный статус.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
    let p = vec![
        ("currency", asset.ticker.clone()),
        ("limit", DEPOSITS_LIMIT.to_string()),
    ];
    let data = rest::signed(ctx, Method::GET, "/wallet/deposits", p, None, Op::Other).await?;
    let rec = data.as_array().and_then(|a| {
        a.iter()
            .find(|r| num::text_opt(r, "txid").is_some_and(|t| tx_hash::same(&t, tx_hash)))
    });
    let rec = rec.ok_or_else(|| {
        let m = format!("Депозит {tx_hash} пока не виден Gate");
        ctx.tag(GatewayError::new(ErrorCode::DepositNotCredited, None, m))
    })?;
    let st = num::text_opt(rec, "status").unwrap_or_default();
    let status = match st.as_str() {
        "DONE" => TransferStatus::Credited,
        "PEND" | "TRACK" | "MANUAL" | "REVIEW" | "DEP_CREDITED" => TransferStatus::Sent,
        "BLOCKED" | "INVALID" => {
            let m = format!("Gate: депозит {tx_hash} не зачислен, статус {st}");
            return Err(ctx.tag(
                GatewayError::new(ErrorCode::DepositNotCredited, None, m).with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(errors::bad(
                &format!("неизвестный статус депозита {other}"),
                &rec.to_string(),
            ));
        }
    };
    // Ссылка депозита — хеш, как у MEXC (ядро ищет депозит по хешу).
    transfer(ctx, rec, tx_hash, status)
}
