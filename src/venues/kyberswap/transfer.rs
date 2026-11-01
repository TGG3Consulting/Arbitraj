//! Файл: venues/kyberswap/transfer.rs
//! Что делает: операции кошелька для площадки kyberswap — «вывод» (перевод монеты из кошелька на адрес биржи, ERC-20
//! transfer), статус вывода и зачисления по квитанции, стоимость перевода (газ + налог перевода), адрес депозита.
//! Монеты — только ERC-20 с адресом контракта (нативный BNB как арбитражная монета не поддержан — см. README).
//! Кто вызывает: venue.rs (withdraw, withdrawal_status, deposit_address, deposit_status, withdraw_cost).
//! От чего зависит: ctx.rs, state.rs, node.rs, tx.rs, abi.rs, num.rs, signer.rs, trade.rs (moved), core/types.
//! Настройки: venue.bsc_network [kyberswap], kyber.assume_zero_transfer_tax [kyberswap], send.deposit_confirmations?
//!            нет — подтверждения депозита берутся из kyber.deposit_confirmations не нужны (зачисление по блоку).

use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::{Address, B256, Bytes, U256};
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{
    Asset, Cost, CostKind, Transfer, TransferStatus, TxOutcome, WithdrawRequest,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::abi::IERC20;
use super::balances::{BNB, decimals};
use super::ctx::Ctx;
use super::state::Parts;
use super::trade::moved;
use super::{node, num, signer, tx};

/// Перевод монеты на адрес (депозит биржи). Ошибки: InvalidAddress, NoRoute (нет адреса токена), сеть, узел,
/// WithdrawRejected / TxNotIncluded / OutcomeUnknown (с фактом транзакции).
pub async fn withdraw(p: &Parts, req: WithdrawRequest) -> GatewayResult<Transfer> {
    let ctx = &p.ctx;
    same_network(ctx, &req.network)?;
    let to: Address = req.address.parse().map_err(|_| {
        ctx.err(
            ErrorCode::InvalidAddress,
            format!("Адрес «{}» — не адрес BSC", req.address),
        )
    })?;
    let token = token(ctx, &req.asset)?;
    let dec = decimals(p, token).await?;
    let units = num::to_units(req.amount, dec).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    })?;
    let _guard = p.wallet_lock.lock().await;
    let wallet = signer::address(ctx)?;
    let data = Bytes::from(IERC20::transferCall { to, value: units }.abi_encode());
    let d = tx::send(ctx, wallet, token, data, U256::ZERO).await?;
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

/// Статус перевода из кошелька по хешу: нет квитанции — Pending, откат — Rejected, успех — Sent (сумма по Transfer
/// из кошелька). Ошибки: узел, хеш не 32 байта, BadResponse.
pub async fn withdrawal_status(p: &Parts, hash: &str) -> GatewayResult<Transfer> {
    let ctx = &p.ctx;
    let wallet = signer::address(ctx)?;
    let h = parse_hash(ctx, hash)?;
    let network = ctx.venue_text("venue.bsc_network")?;
    let Some(r) = node::receipt(ctx, h).await? else {
        return Ok(pending(ctx, h, network));
    };
    if r.status.to::<u64>() != 1 {
        return Ok(rejected(ctx, h, network));
    }
    // Токен — по первому событию Transfer из кошелька.
    let token = r.logs.iter().find_map(|l| {
        let t = IERC20::Transfer::decode_log_data(&l.inner.data).ok()?;
        (t.from == wallet).then(|| l.address())
    });
    let (asset, units, dec) = match token {
        Some(t) => {
            let ticker = p
                .assets()
                .into_iter()
                .find(|a| a.bsc_contract == Some(t))
                .map(|a| a.ticker)
                .unwrap_or_default();
            (
                Asset {
                    ticker,
                    bsc_contract: Some(t),
                },
                moved(&r, t, Some(wallet), None),
                decimals(p, t).await?,
            )
        }
        None => {
            return Err(ctx.err(
                ErrorCode::BadResponse,
                format!("Транзакция {h} в блоке, но в ней нет перевода токена из кошелька"),
            ));
        }
    };
    let amount = num::to_decimal(units, dec).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Сумма {units} в {h} не переводится в число"),
        )
    })?;
    let fee = r
        .gas_bnb()
        .map(|g| Cost::new(CostKind::TransferGas, g, BNB, false));
    Ok(transfer(
        ctx,
        asset,
        network,
        amount,
        TransferStatus::Sent,
        h,
        fee,
    ))
}

/// Статус зачисления монеты в кошелёк по хешу: Transfer монеты на адрес кошелька и kyber.deposit_confirmations блоков
/// после блока перевода — Credited, раньше — Pending. Ошибки: узел, NotConfigured, BadResponse.
pub async fn deposit_status(p: &Parts, asset: &Asset, hash: &str) -> GatewayResult<Transfer> {
    let ctx = &p.ctx;
    let wallet = signer::address(ctx)?;
    let h = parse_hash(ctx, hash)?;
    let network = ctx.venue_text("venue.bsc_network")?;
    let token = token(ctx, asset)?;
    let Some(r) = node::receipt(ctx, h).await? else {
        return Ok(pending(ctx, h, network));
    };
    if r.status.to::<u64>() != 1 {
        return Ok(rejected(ctx, h, network));
    }
    let units = moved(&r, token, None, Some(wallet));
    if units.is_zero() {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            format!(
                "Транзакция {h} в блоке, но перевода {} на кошелёк нет",
                asset.ticker
            ),
        ));
    }
    let dec = decimals(p, token).await?;
    let amount = num::to_decimal(units, dec).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Сумма {units} в {h} не переводится в число"),
        )
    })?;
    let need = ctx.int("kyber.deposit_confirmations")?;
    let now = node::block_number(ctx).await?;
    let block: u64 = r.block_number.to();
    let st = if now >= block + need {
        TransferStatus::Credited
    } else {
        TransferStatus::Pending
    };
    Ok(transfer(ctx, asset.clone(), network, amount, st, h, None))
}

/// Стоимость перевода: газ (оценка перевода × цена газа) и налог перевода. Налог перевода агрегатор не измеряет:
/// только при kyber.assume_zero_transfer_tax = да он равен нулю; иначе CoinNotChecked (сделки нет).
/// Ошибки: сеть, узел, CoinNotChecked.
pub async fn withdraw_cost(
    p: &Parts,
    asset: &Asset,
    network: &str,
    amount: Decimal,
) -> GatewayResult<Vec<Cost>> {
    let ctx = &p.ctx;
    same_network(ctx, network)?;
    if !ctx
        .venue_flag("kyber.assume_zero_transfer_tax")
        .unwrap_or(false)
    {
        return Err(ctx.err(
            ErrorCode::CoinNotChecked,
            format!(
                "{}: налог перевода не измеряется агрегатором — включите kyber.assume_zero_transfer_tax для выверенных монет",
                asset.ticker
            ),
        ));
    }
    let token = token(ctx, asset)?;
    let dec = decimals(p, token).await?;
    let units = num::to_units(amount, dec).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    })?;
    let wallet = signer::address(ctx)?;
    let data = Bytes::from(
        IERC20::transferCall {
            to: wallet,
            value: units,
        }
        .abi_encode(),
    );
    let gas = node::estimate_gas(ctx, wallet, token, &data, U256::ZERO).await?;
    let price = node::gas_price(ctx).await?;
    let wei = price.saturating_mul(U256::from(gas));
    let bnb = num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Газ перевода {wei} wei не переводится в BNB"),
        )
    })?;
    // Налог перевода принят равным нулю по явной настройке — отдельной строкой не добавляется.
    Ok(vec![Cost::new(CostKind::TransferGas, bnb, BNB, false)])
}

/// Адрес токена монеты. Ошибки: NoRoute — у монеты нет адреса в BSC.
fn token(ctx: &Ctx, a: &Asset) -> GatewayResult<Address> {
    a.bsc_contract.ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            format!("{}: нет адреса контракта в BSC", a.ticker),
        )
    })
}

/// Сеть запроса — сеть BSC (venue.bsc_network). Ошибки: NotConfigured, InvalidAddress (другая сеть).
fn same_network(ctx: &Ctx, network: &str) -> GatewayResult<()> {
    let ours = ctx.venue_text("venue.bsc_network")?;
    if network != ours {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            format!("Кошелёк kyberswap работает только в сети {ours}, запрошена {network}"),
        ));
    }
    Ok(())
}

/// Хеш транзакции из строки. Ошибки: BadResponse — не 32 байта.
fn parse_hash(ctx: &Ctx, hash: &str) -> GatewayResult<B256> {
    hash.parse().map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("«{hash}» — не хеш транзакции"),
        )
    })
}

/// Перевод «в ожидании». Ошибок нет.
fn pending(ctx: &Ctx, h: B256, network: String) -> Transfer {
    transfer(
        ctx,
        empty_asset(),
        network,
        Decimal::ZERO,
        TransferStatus::Pending,
        h,
        None,
    )
}

/// Перевод «отклонён». Ошибок нет.
fn rejected(ctx: &Ctx, h: B256, network: String) -> Transfer {
    transfer(
        ctx,
        empty_asset(),
        network,
        Decimal::ZERO,
        TransferStatus::Rejected,
        h,
        None,
    )
}

/// Монета без данных (для статусов до разбора логов). Ошибок нет.
fn empty_asset() -> Asset {
    Asset {
        ticker: String::new(),
        bsc_contract: None,
    }
}

/// Перевод в формате ядра. Ошибок нет.
fn transfer(
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
