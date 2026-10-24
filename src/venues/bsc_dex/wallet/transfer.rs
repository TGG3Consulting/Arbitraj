//! Файл: venues/bsc_dex/wallet/transfer.rs
//! Что делает: «вывод» для DEX — перевод монеты из кошелька бота на адрес (депозит биржи) транзакцией ERC-20 transfer
//! через каналы отправки и ожидание итога своим узлом. Стоимость перевода — wallet/transfer_cost.rs, статусы по
//! квитанции — wallet/transfer_status.rs.
//! Нативная монета BNB (решение 62, exec/native.rs): перевод — нативный (value на адрес депозита, без данных), только
//! сверх резерва на газ risk.min_bnb_balance.
//! Кто вызывает: mod.rs (контракт withdraw), wallet/transfer_status.rs, wallet/transfer_cost.rs (стоимость перевода).
//! От чего зависит: exec/txs.rs, exec/native.rs, exec/fill.rs, send/track.rs, tokens.rs, num.rs, wallet/nonce.rs.
//! Настройки: venue.bsc_network [bsc_dex] (какую сеть принимаем в запросе); через вызываемые — send.*, channel.*.
//!
//! transfer(to, value), событие Transfer — https://eips.ethereum.org/EIPS/eip-20 ; квитанция —
//! execution-apis src/eth/transaction.yaml#L127-L165 (research/flow/bsc_send.md, раздел 5).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{
    Asset, Cost, CostKind, Transfer, TransferStatus, TxOutcome, WithdrawRequest,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::native;
use super::super::exec::txs::{self, Env};
use super::super::node::abi::IERC20;
use super::super::num;
use super::super::tokens::Tokens;
use super::balances::BNB;
use super::nonce::Nonces;

/// Перевод монеты на адрес. Ошибки: адрес, сеть, нет адреса токена, каналы, не попала / откат (с фактом транзакции),
/// OutcomeUnknown — nonce занят без своей квитанции или очередь остановлена (повторять вывод нельзя).
pub async fn withdraw(
    env: &Env,
    tokens: &Tokens,
    nonces: &Nonces,
    req: WithdrawRequest,
) -> GatewayResult<Transfer> {
    let ctx = &env.ctx;
    same_network(ctx, &req.network)?;
    let to: Address = req.address.parse().map_err(|_| {
        ctx.err(
            ErrorCode::InvalidAddress,
            format!("Адрес «{}» — не адрес BSC", req.address),
        )
    })?;
    let token = token(ctx, &req.asset)?;
    let native = native::is_native(ctx, &req.asset);
    let dec = if native {
        num::BNB_DECIMALS
    } else {
        tokens.decimals(ctx, token).await?
    };
    let units = num::to_units(req.amount, dec).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    })?;
    let slot = nonces.acquire(ctx).await?;
    let (target, data, value) = if native {
        native::reserve_ok(ctx, slot.from, units).await?;
        (to, Bytes::new(), units)
    } else {
        let data = IERC20::transferCall { to, value: units }.abi_encode();
        (token, Bytes::from(data), U256::ZERO)
    };
    let d = txs::send(env, &slot, target, data, value, false, None).await?;
    drop(slot);
    match d.chain.outcome {
        TxOutcome::Included => {}
        TxOutcome::Reverted => {
            return Err(ctx
                .err(
                    ErrorCode::WithdrawRejected,
                    format!("Перевод откатился в блоке, tx {}", d.chain.hash),
                )
                .with_tx(d.chain));
        }
        TxOutcome::Replaced => {
            // Nonce занят, своей квитанции нет: перевод мог уйти — повтор вывода опасен, только ручная проверка.
            return Err(ctx
                .err(
                    ErrorCode::OutcomeUnknown,
                    format!("Исход перевода tx {} неизвестен (nonce занят, квитанции нет) — нужна ручная проверка", d.chain.hash),
                )
                .with_tx(d.chain));
        }
        _ => {
            return Err(ctx
                .err(
                    ErrorCode::TxNotIncluded,
                    format!("Перевод не попал в блок, tx {}", d.chain.hash),
                )
                .with_tx(d.chain));
        }
    }
    let fee = d
        .chain
        .gas_bnb
        .map(|g| Cost::new(CostKind::TransferGas, g, BNB, false));
    Ok(transfer(
        ctx,
        req.asset,
        req.network,
        req.amount,
        TransferStatus::Sent,
        d.chain.hash,
        fee,
    ))
}

/// Адрес токена монеты. Ошибки: NotConfigured — у монеты нет адреса в BSC.
pub fn token(ctx: &Ctx, a: &Asset) -> GatewayResult<Address> {
    a.bsc_contract.ok_or_else(|| {
        ctx.err(
            ErrorCode::NotConfigured,
            format!("{}: нет адреса контракта в BSC", a.ticker),
        )
    })
}

/// Сеть запроса — сеть BSC (venue.bsc_network). Ошибки: NotConfigured, InvalidAddress (другая сеть).
pub fn same_network(ctx: &Ctx, network: &str) -> GatewayResult<()> {
    let ours = ctx.text("venue.bsc_network")?;
    if network != ours {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            format!("Кошелёк DEX работает только в сети {ours}, запрошена {network}"),
        ));
    }
    Ok(())
}

/// Перевод в формате ядра. Ошибок нет.
pub fn transfer(
    ctx: &Ctx,
    asset: Asset,
    network: String,
    amount: Decimal,
    status: TransferStatus,
    hash: B256,
    fee: Option<Cost>,
) -> Transfer {
    Transfer {
        venue: ctx.venue(),
        asset,
        network,
        amount,
        status,
        venue_ref: hash.to_string(),
        tx_hash: Some(hash.to_string()),
        fee,
        updated_at: Utc::now(),
    }
}
