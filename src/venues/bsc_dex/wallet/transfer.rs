//! Файл: venues/bsc_dex/wallet/transfer.rs
//! Что делает: «вывод» для DEX — перевод монеты из кошелька бота на адрес (депозит биржи) транзакцией ERC-20 transfer
//! через каналы отправки и ожидание итога своим узлом; стоимость перевода — газ (измерен пробным переводом) и налог
//! на перевод (проверка монеты). Статусы по квитанции — wallet/transfer_status.rs.
//! Кто вызывает: mod.rs (контракт withdraw, withdraw_cost), wallet/transfer_status.rs.
//! От чего зависит: exec/txs.rs, exec/fill.rs, send/track.rs, check/state.rs, quote/gas.rs, tokens.rs, num.rs, wallet/nonce.rs.
//! Настройки: venue.bsc_network [bsc_dex] (какую сеть принимаем в запросе); через вызываемые — send.*, channel.*.
//!
//! transfer(to, value), событие Transfer — https://eips.ethereum.org/EIPS/eip-20 ; квитанция —
//! execution-apis src/eth/transaction.yaml#L127-L165 (research/flow/bsc_send.md, раздел 5).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{
    Asset, CheckState, Cost, CostKind, Transfer, TransferStatus, TxOutcome, WithdrawRequest,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::state::Checks;
use super::super::ctx::Ctx;
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
    let units = num::to_units(req.amount, tokens.decimals(ctx, token).await?).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    })?;
    let slot = nonces.acquire(ctx).await?;
    let data = Bytes::from(IERC20::transferCall { to, value: units }.abi_encode());
    let d = txs::send(env, &slot, token, data, U256::ZERO, false, None).await?;
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

/// Стоимость перевода: газ (пробный перевод × цена газа) и налог на перевод (% проверки монеты).
/// Ошибки: CoinNotChecked / CoinCheckFailed, сеть, узел (цена газа).
pub async fn cost(
    env: &Env,
    checks: &Checks,
    asset: &Asset,
    network: &str,
    amount: Decimal,
) -> GatewayResult<Vec<Cost>> {
    let ctx = &env.ctx;
    same_network(ctx, network)?;
    let pct = match checks.coin(&asset.ticker).map(|c| c.taxes) {
        Some(CheckState::Known(t)) => t.transfer_pct,
        Some(CheckState::Failed { reason }) => {
            return Err(ctx.err(ErrorCode::CoinCheckFailed, reason));
        }
        _ => {
            return Err(ctx.err(
                ErrorCode::CoinNotChecked,
                format!("{}: налоги не проверены", asset.ticker),
            ));
        }
    };
    let gas = checks.transfer_gas(&asset.ticker).ok_or_else(|| {
        ctx.err(
            ErrorCode::CoinNotChecked,
            format!("{}: газ перевода не измерен", asset.ticker),
        )
    })?;
    let price = env.gas.wei(ctx, &env.heads).await?;
    let wei = price.saturating_mul(U256::from(gas));
    let bnb = num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Газ перевода {wei} wei не переводится в BNB"),
        )
    })?;
    Ok(vec![
        Cost::new(CostKind::TransferGas, bnb, BNB, false),
        Cost::new(
            CostKind::TransferTax,
            amount * pct / Decimal::ONE_HUNDRED,
            &asset.ticker,
            false,
        ),
    ])
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
