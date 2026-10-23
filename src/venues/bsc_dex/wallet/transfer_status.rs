//! Файл: venues/bsc_dex/wallet/transfer_status.rs
//! Что делает: статус перевода из кошелька (вывода) и зачисления в кошелёк (депозита) по квитанции транзакции:
//! нет квитанции — Pending, откат — Rejected, успех — Sent (вывод) / Credited (депозит: ненулевой Transfer на кошелёк и
//! send.deposit_confirmations блоков после блока перевода; раньше — Pending), сумма — по событиям Transfer.
//! Кто вызывает: mod.rs (контракт withdrawal_status, deposit_status).
//! От чего зависит: node/rpc.rs, send/track.rs (Receipt), exec/fill.rs, wallet/transfer.rs, tokens.rs, num.rs.
//! Настройки: venue.bsc_network, send.deposit_confirmations [bsc_dex].
//!
//! Квитанция — execution-apis src/eth/transaction.yaml#L127-L165; eth_blockNumber — execution-apis src/eth/client.yaml (16caee27);
//! Transfer — https://eips.ethereum.org/EIPS/eip-20 .

use alloy_primitives::{Address, B256, U64};
use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::fill;
use super::super::node::rpc;
use super::super::num;
use super::super::send::track::Receipt;
use super::super::tokens::Tokens;
use super::balances::BNB;
use super::transfer::{token, transfer};

/// Статус перевода из кошелька по хешу: нет квитанции — Pending, откат — Rejected, успех — Sent.
/// Ошибки: узел, хеш не 32 байта.
pub async fn withdrawal_status(
    ctx: &Ctx,
    tokens: &Tokens,
    hash: &str,
    wallet: Address,
) -> GatewayResult<Transfer> {
    status(ctx, tokens, hash, wallet, None).await
}

/// Зачисление в кошелёк по хешу: Transfer монеты на адрес кошелька → Credited. Ошибки: как у withdrawal_status.
pub async fn deposit_status(
    ctx: &Ctx,
    tokens: &Tokens,
    asset: &Asset,
    hash: &str,
    wallet: Address,
) -> GatewayResult<Transfer> {
    status(ctx, tokens, hash, wallet, Some(asset)).await
}

/// Общий разбор квитанции для статусов.
/// Вывод — токен по событию Transfer «из кошелька»; депозит — заданная монета «на кошелёк»: «Зачислен» только при
/// ненулевой сумме Transfer на кошелёк и когда после блока перевода прошло send.deposit_confirmations блоков.
/// Ошибки: узел, хеш не 32 байта, NotConfigured, BadResponse (успешная транзакция без перевода токена кошелька;
/// сумма не переводится в число — с сырым значением).
async fn status(
    ctx: &Ctx,
    tokens: &Tokens,
    hash: &str,
    wallet: Address,
    deposit: Option<&Asset>,
) -> GatewayResult<Transfer> {
    let h: B256 = hash.parse().map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("«{hash}» — не хеш транзакции"),
        )
    })?;
    let r: Option<Receipt> = rpc::call(ctx, "eth_getTransactionReceipt", json!([h])).await?;
    let network = ctx.text("venue.bsc_network")?;
    let (asset, token) = match deposit {
        Some(a) => (a.clone(), Some(token(ctx, a)?)),
        None => {
            let t = r.as_ref().and_then(|r| fill::token_from(r, wallet));
            let found = t.and_then(|t| tokens.all().into_iter().find(|x| x.address == t));
            let ticker = found.map(|x| x.ticker).unwrap_or_default();
            (
                Asset {
                    ticker,
                    bsc_contract: t,
                },
                t,
            )
        }
    };
    let state = |a: Asset, st: TransferStatus, amount: Decimal| {
        transfer(ctx, a, network.clone(), amount, st, h, None)
    };
    let Some(r) = r else {
        return Ok(state(asset, TransferStatus::Pending, Decimal::ZERO));
    };
    if r.status.to::<u64>() != 1 {
        return Ok(state(asset, TransferStatus::Rejected, Decimal::ZERO));
    }
    let Some(token) = token else {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            format!("Транзакция {h} в блоке, но в ней нет перевода токена из кошелька"),
        ));
    };
    let units = match deposit {
        Some(_) => fill::moved(&r, token, None, Some(wallet)),
        None => fill::moved(&r, token, Some(wallet), None),
    };
    if units.is_zero() {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            format!(
                "Транзакция {h} в блоке, но перевода {} на кошелёк/с кошелька нет (сумма 0)",
                asset.ticker
            ),
        ));
    }
    let dec = tokens.decimals(ctx, token).await?;
    let amount = num::to_decimal(units, dec).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Сумма перевода {units} (decimals {dec}) в {h} не переводится в число"),
        )
    })?;
    if deposit.is_none() {
        let fee = r
            .gas_bnb()
            .map(|g| Cost::new(CostKind::TransferGas, g, BNB, false));
        return Ok(transfer(
            ctx,
            asset,
            network,
            amount,
            TransferStatus::Sent,
            h,
            fee,
        ));
    }
    let need = ctx.int("send.deposit_confirmations")?;
    let now: U64 = rpc::call(ctx, "eth_blockNumber", json!([])).await?;
    let block: u64 = r.block_number.to();
    let st = if now.to::<u64>() >= block + need {
        TransferStatus::Credited
    } else {
        TransferStatus::Pending
    };
    Ok(state(asset, st, amount))
}
