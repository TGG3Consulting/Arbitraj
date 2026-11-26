//! Файл: venues/bsc_dex/wallet/transfer_status.rs
//! Что делает: статус перевода из кошелька (вывода) и зачисления в кошелёк (депозита) по квитанции транзакции:
//! нет квитанции — Pending, откат — Rejected, успех — Sent (вывод) / Credited (депозит: ненулевой Transfer на кошелёк и
//! send.deposit_confirmations блоков после блока перевода; раньше — Pending), сумма — по событиям Transfer.
//! Нативная монета BNB (решение 62): сумма и получатель — из самой транзакции (wallet/native_status.rs).
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
use super::native_status;
use super::transfer::{token, transfer};

/// Статус перевода из кошелька по хешу: нет квитанции — Pending, откат — Rejected, успех — Sent.
/// Ошибки: узел, хеш не 32 байта.
pub async fn withdrawal_status(
    ctx: &Ctx,
    tokens: &Tokens,
    hash: &str,
    wallet: Address,
) -> GatewayResult<Transfer> {
    status(ctx, tokens, hash, wallet, None, Proof::default()).await
}

/// Зачисление в кошелёк по хешу: Transfer монеты на адрес кошелька → Credited. Ошибки: как у withdrawal_status.
pub async fn deposit_status(
    ctx: &Ctx,
    tokens: &Tokens,
    asset: &Asset,
    hash: &str,
    wallet: Address,
) -> GatewayResult<Transfer> {
    deposit_for(ctx, tokens, asset, hash, wallet, Proof::default()).await
}

#[derive(Default)]
pub(super) struct Proof<'a> {
    pub claim: Option<&'a crate::gateway::contract::DepositClaim>,
    pub event_amount: Option<Decimal>,
    pub confirmed: Option<&'a Receipt>,
}

pub(super) async fn deposit_for(
    ctx: &Ctx,
    tokens: &Tokens,
    asset: &Asset,
    hash: &str,
    wallet: Address,
    proof: Proof<'_>,
) -> GatewayResult<Transfer> {
    status(ctx, tokens, hash, wallet, Some(asset), proof).await
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
    proof: Proof<'_>,
) -> GatewayResult<Transfer> {
    let h: B256 = hash.parse().map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("«{hash}» — не хеш транзакции"),
        )
    })?;
    let fetched;
    let r = match proof.confirmed {
        Some(receipt) => Some(receipt),
        None => {
            fetched = super::super::send::receipt::read(ctx, h).await?;
            fetched.as_ref()
        }
    };
    let network = ctx.text("venue.bsc_network")?;
    let (asset, token) = match deposit {
        Some(a) => (a.clone(), Some(token(ctx, a)?)),
        None => {
            let t = r.as_ref().and_then(|r| fill::token_from(r, wallet));
            let found = t.and_then(|t| tokens.all().into_iter().find(|x| x.address == t));
            let ticker = found.map(|x| x.ticker).unwrap_or_default();
            (
                Asset {
                    chain: None,
                    ticker,
                    contract: t,
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
    // Нативная BNB (решение 62): сумма — value самой транзакции, событий Transfer нет (wallet/native_status.rs).
    let native = native_status::check(ctx, h, wallet, deposit, token.is_some()).await?;
    let mut log_index = None;
    let (asset, units, dec) = match (native, token) {
        (Some((a, wei)), _) => (a, wei, num::BNB_DECIMALS),
        (None, None) => {
            return Err(ctx.err(
                ErrorCode::BadResponse,
                format!("Транзакция {h} в блоке, но в ней нет перевода токена из кошелька"),
            ));
        }
        (None, Some(token)) => {
            let decimals = tokens.decimals(ctx, token).await?;
            let units = if deposit.is_some() {
                let exact = proof.claim.and_then(|c| c.expected).and_then(|v| {
                    num::to_units(v, decimals)
                        .filter(|units| num::to_decimal(*units, decimals) == Some(v))
                });
                let incoming = super::deposit_log::incoming(r, token, wallet, h)
                    .map_err(|why| ctx.err(ErrorCode::DepositNotCredited, why))?;
                let (index, units) = match super::deposit_log::select(&incoming, exact) {
                    Ok(hit) => hit,
                    Err(why) => {
                        let error = || ctx.err(ErrorCode::DepositNotCredited, why);
                        let claim = proof.claim.ok_or_else(error)?;
                        super::deposit_log::previous(ctx, h, token, wallet, claim, &incoming)
                            .await?
                            .ok_or_else(error)?
                    }
                };
                log_index = Some(index);
                units
            } else {
                fill::moved(r, token, Some(wallet), None)
            };
            (asset, units, decimals)
        }
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
    let amount = num::to_decimal(units, dec).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Сумма перевода {units} (decimals {dec}) в {h} не переводится в число"),
        )
    })?;
    if deposit.is_none() {
        // Решение 416: тикер газа — dex.gas_ticker (в памяти после отправки этим процессом, иначе из настроек).
        let gas_t = super::super::gas_ticker::sent(ctx)?;
        let fee = r
            .gas_bnb()
            .map(|g| Cost::new(CostKind::TransferGas, g, &gas_t, false));
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
    let confirmed = if proof.confirmed.is_some() {
        true
    } else {
        let need = ctx.int("send.deposit_confirmations")?;
        let now: U64 = rpc::call(ctx, "eth_blockNumber", json!([])).await?;
        now.to::<u64>().saturating_sub(r.block_number.to()) >= need && now >= r.block_number
    };
    let st = if confirmed {
        TransferStatus::Credited
    } else {
        TransferStatus::Pending
    };
    let mut credit = state(asset, st, amount);
    if st == TransferStatus::Credited
        && let (Some(index), Some(claim), Some(token)) = (log_index, proof.claim, token)
    {
        if proof.event_amount.is_some_and(|delta| delta != amount) {
            return Err(ctx.err(
                ErrorCode::DepositNotCredited,
                "Сумма события отличается от перевода в квитанции",
            ));
        }
        super::deposit_log::bind(ctx, h, token, wallet, index, units, claim).await?;
        credit.venue_ref = format!("{h}#{index}");
    }
    Ok(credit)
}
