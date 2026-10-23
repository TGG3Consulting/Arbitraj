//! Файл: venues/bsc_dex/exec/txs.rs
//! Что делает: одна транзакция кошелька от начала до итога: каналы (send/mod.rs), цена газа (eth_gasPrice + надбавка,
//! не ниже минимума выбранных каналов), лимит газа (eth_estimateGas — заодно проверка, что вызов не откатится, —
//! плюс запас), подпись (EIP-155, legacy), чаевые (только один канал и бандл), отправка, ожидание итога своим узлом.
//! Nonce — текущий счётчик кошелька в блоках: транзакции идут строго по очереди внутри места в очереди (решение 45).
//! Кто вызывает: exec/trade.rs, wallet/transfer.rs.
//! От чего зависит: send/*, quote/gas.rs, wallet/signer.rs, node/rpc.rs, node/heads.rs, num.rs.
//! Настройки: send.gas_limit_margin_pct, send.bundle_max_blocks [bsc_dex]; через вызываемые — channel.*, send.*.
//!
//! eth_estimateGas — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/execute.yaml#L43-L60 ;
//! legacy-транзакция с chainId (EIP-155) — https://eips.ethereum.org/EIPS/eip-155 ; база газа в BSC всегда 0 —
//! research/flow/bsc_dex.md, раздел 5 (поэтому цена газа — одна gasPrice).

use alloy::consensus::TxLegacy;
use alloy_primitives::{Address, B256, Bytes, TxKind, U64, U256};
use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::{ChainTx, SendMethod, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::BSC_CHAIN_ID;
use super::super::node::heads::Heads;
use super::super::node::rpc;
use super::super::num;
use super::super::quote::gas::GasPrice;
use super::super::send::stats::Stats;
use super::super::send::track::{self, Receipt};
use super::super::send::{self};
use super::super::wallet::signer;

/// Всё для отправки. Дешёво клонируется.
#[derive(Clone)]
pub struct Env {
    pub ctx: Ctx,
    pub heads: Heads,
    pub stats: Stats,
    pub gas: GasPrice,
}

/// Итог транзакции.
pub struct Done {
    pub chain: ChainTx,
    pub receipt: Option<Receipt>,
}

/// Отправляет вызов и ждёт итога. Принимает: окружение, отправителя, адрес, данные, сумму BNB (wei).
/// Возвращает: Done (итог может быть «откат» / «не попала» — решает вызывающий).
/// Ошибки: настройки, нет каналов, eth_estimateGas откатился (текст узла — транзакция не отправлялась), каналы не приняли.
pub async fn send(
    env: &Env,
    from: Address,
    to: Address,
    data: Bytes,
    value: U256,
) -> GatewayResult<Done> {
    let ctx = &env.ctx;
    let ready = send::select(ctx, &env.stats)?;
    let price = env
        .gas
        .wei(ctx, &env.heads)
        .await?
        .max(send::gas_floor(&ready));
    let nonce: U64 = rpc::call(ctx, "eth_getTransactionCount", json!([from, "latest"])).await?;
    let nonce = nonce.to::<u64>();
    let gas = limit(ctx, from, to, &data, value).await?;
    let tx = legacy(nonce, price, gas, to, value, data);
    let signed = signer::sign(ctx, tx)?;
    let tip = match send::tip(&ready) {
        Some((tip_to, amount)) => {
            let g = limit(ctx, from, tip_to, &Bytes::new(), amount).await?;
            Some((
                signer::sign(
                    ctx,
                    legacy(nonce + 1, price, g, tip_to, amount, Bytes::new()),
                )?,
                amount,
            ))
        }
        None => None,
    };
    let head = env.heads.fresh(ctx)?;
    let max_block = head.number + ctx.int("send.bundle_max_blocks")?;
    let channels = send::dispatch(
        ctx,
        &env.stats,
        &ready,
        &signed.raw,
        tip.as_ref().map(|t| &t.0.raw),
        max_block,
    )
    .await?;
    let method = if ready.iter().all(|r| r.method == SendMethod::Bundle) {
        SendMethod::Bundle
    } else {
        SendMethod::Single
    };
    let t = track::wait(
        ctx,
        &env.heads,
        &env.stats,
        signed.hash,
        nonce,
        from,
        head.number,
        &channels,
    )
    .await?;
    let tip_bnb = tip.and_then(|(_, a)| num::to_decimal(a, num::BNB_DECIMALS));
    Ok(Done {
        chain: chain_tx(signed.hash, nonce, channels, method, &t, tip_bnb),
        receipt: t.receipt,
    })
}

/// Лимит газа: eth_estimateGas × (1 + send.gas_limit_margin_pct/100). Ошибки: откат (текст узла), настройка.
async fn limit(
    ctx: &Ctx,
    from: Address,
    to: Address,
    data: &Bytes,
    value: U256,
) -> GatewayResult<u64> {
    let est: U64 = rpc::call(
        ctx,
        "eth_estimateGas",
        json!([{ "from": from, "to": to, "input": data, "value": value }]),
    )
    .await?;
    let margin = ctx.dec("send.gas_limit_margin_pct")?;
    let g = Decimal::from(est.to::<u64>()) * (Decimal::ONE + margin / Decimal::ONE_HUNDRED);
    u64::try_from(g.ceil()).map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Лимит газа {g} не помещается"),
        )
    })
}

/// Legacy-транзакция с chainId BSC (EIP-155). Ошибок нет.
fn legacy(nonce: u64, price: U256, gas: u64, to: Address, value: U256, input: Bytes) -> TxLegacy {
    TxLegacy {
        chain_id: Some(BSC_CHAIN_ID),
        nonce,
        gas_price: price.saturating_to::<u128>(),
        gas_limit: gas,
        to: TxKind::Call(to),
        value,
        input,
    }
}

/// ChainTx из итога ожидания. Ошибок нет.
fn chain_tx(
    hash: B256,
    nonce: u64,
    channels: Vec<String>,
    method: SendMethod,
    t: &track::Tracked,
    tip_bnb: Option<Decimal>,
) -> ChainTx {
    let r = t.receipt.as_ref();
    ChainTx {
        hash,
        nonce,
        channels,
        method,
        outcome: t.outcome,
        block: r.map(|r| r.block_number.to()),
        gas_used: r.map(|r| r.gas_used.to()),
        gas_bnb: r.and_then(|r| r.gas_bnb()),
        tip_bnb: if t.outcome == TxOutcome::Included || t.outcome == TxOutcome::Reverted {
            tip_bnb
        } else {
            None
        },
    }
}
