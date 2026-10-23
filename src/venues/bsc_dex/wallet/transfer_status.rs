//! Файл: venues/bsc_dex/wallet/transfer_status.rs
//! Что делает: статус перевода из кошелька (вывода) и зачисления в кошелёк (депозита) по квитанции транзакции:
//! нет квитанции — Pending, откат — Rejected, успех — Sent (вывод) / Credited (депозит), сумма — по событиям Transfer.
//! Кто вызывает: mod.rs (контракт withdrawal_status, deposit_status).
//! От чего зависит: node/rpc.rs, send/track.rs (Receipt), exec/fill.rs, wallet/transfer.rs, tokens.rs, num.rs.
//! Настройки: venue.bsc_network [bsc_dex].
//!
//! Квитанция — execution-apis src/eth/transaction.yaml#L127-L165; Transfer — https://eips.ethereum.org/EIPS/eip-20 .

use alloy_primitives::{Address, B256};
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
/// Вывод — токен по событию Transfer «из кошелька»; депозит — заданная монета «на кошелёк».
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
        Some(a) => (a.clone(), token(ctx, a)?),
        None => {
            let t = r.as_ref().and_then(|r| fill::token_from(r, wallet));
            let found = t.and_then(|t| tokens.all().into_iter().find(|x| x.address == t));
            match found {
                Some(x) => (
                    Asset {
                        ticker: x.ticker,
                        bsc_contract: Some(x.address),
                    },
                    x.address,
                ),
                None => (
                    Asset {
                        ticker: String::new(),
                        bsc_contract: t,
                    },
                    t.unwrap_or_default(),
                ),
            }
        }
    };
    let Some(r) = r else {
        return Ok(transfer(
            ctx,
            asset,
            network,
            Decimal::ZERO,
            TransferStatus::Pending,
            h,
            None,
        ));
    };
    if r.status.to::<u64>() != 1 {
        return Ok(transfer(
            ctx,
            asset,
            network,
            Decimal::ZERO,
            TransferStatus::Rejected,
            h,
            None,
        ));
    }
    let units = match deposit {
        Some(_) => fill::moved(&r, token, None, Some(wallet)),
        None => fill::moved(&r, token, Some(wallet), None),
    };
    let amount = num::to_decimal(units, tokens.decimals(ctx, token).await?).unwrap_or_default();
    let st = if deposit.is_some() {
        TransferStatus::Credited
    } else {
        TransferStatus::Sent
    };
    let fee = if deposit.is_none() {
        r.gas_bnb()
            .map(|g| Cost::new(CostKind::TransferGas, g, BNB, false))
    } else {
        None
    };
    Ok(transfer(ctx, asset, network, amount, st, h, fee))
}
