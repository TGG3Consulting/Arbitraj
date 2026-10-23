//! Файл: venues/bsc_dex/exec/txs.rs
//! Что делает: одна транзакция кошелька от начала до итога: каналы (send/mod.rs), цена газа (eth_gasPrice + надбавка,
//! не ниже минимума выбранных каналов), лимит газа (eth_estimateGas — заодно проверка, что вызов не откатится, —
//! плюс запас), подпись (EIP-155, legacy), чаевые (только по просьбе вызывающего — один раз на сделку, к транзакции
//! обмена; и только один канал и бандл), отправка, ожидание итога своим узлом до занятия nonce в блоке.
//! Ожидание идёт в отдельной задаче, которая держит место в очереди кошелька (wallet/nonce.rs, Hold): даже если
//! вызывающий прерван, следующая транзакция кошелька не возьмёт nonce, пока этот не занят (решение 45).
//! Кто вызывает: exec/trade.rs, wallet/transfer.rs.
//! От чего зависит: send/*, quote/gas.rs, wallet/signer.rs, wallet/nonce.rs, wallet/inflows.rs, node/heads.rs, num.rs.
//! Настройки: send.* через send/tx.rs (SendCfg — читается до отправки), channel.*.
//!
//! eth_estimateGas, legacy-транзакция EIP-155 — ссылки в send/tx.rs; трекер и отмена — send/track.rs, send/cancel.rs.

use alloy_primitives::{Address, B256, Bytes, U256};
use rust_decimal::Decimal;

use crate::core::types::{ChainTx, SendMethod, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::num;
use super::super::quote::gas::GasPrice;
use super::super::send::settle::{self, After};
use super::super::send::stats::Stats;
use super::super::send::track::{self, Receipt, Tracked, Watch};
use super::super::send::tx::{self, SendCfg};
use super::super::send::{self};
use super::super::wallet::inflows::{Inflows, SwapGuard};
use super::super::wallet::nonce::Slot;
use super::super::wallet::signer;

/// Всё для отправки. Дешёво клонируется.
#[derive(Clone)]
pub struct Env {
    pub ctx: Ctx,
    pub heads: Heads,
    pub stats: Stats,
    pub gas: GasPrice,
    /// Ожидаемые поступления в кошелёк и идущие обмены (wallet/inflows.rs).
    pub inflows: Inflows,
}

/// Итог транзакции (nonce занят в блоке).
pub struct Done {
    pub chain: ChainTx,
    pub receipt: Option<Receipt>,
    /// Отмена, занявшая nonce (исходная не попала): хеш и квитанция.
    pub cancel: Option<(B256, Receipt)>,
}

/// Отправляет вызов и ждёт, пока его nonce не занят в блоке. Принимает: окружение, место в очереди кошелька, адрес,
/// данные, сумму BNB (wei), чаевые (true — только для транзакции обмена), отметку «идёт обмен» (wallet/inflows.rs —
/// снимается вместе с местом в очереди, когда nonce занят).
/// Возвращает: Done (итог может быть «откат» / «не попала» / «вытеснена» — решает вызывающий).
/// Ошибки (транзакция не ушла ни в один канал): настройки, нет каналов, eth_estimateGas откатился (текст узла),
/// все каналы ответили отказом; Internal — задача трекера упала.
pub async fn send(
    env: &Env,
    slot: &Slot,
    to: Address,
    data: Bytes,
    value: U256,
    with_tip: bool,
    keep: Option<SwapGuard>,
) -> GatewayResult<Done> {
    let ctx = &env.ctx;
    let cfg = SendCfg::read(ctx)?;
    let ready = send::select(ctx, &env.stats)?;
    let price = env
        .gas
        .wei(ctx, &env.heads)
        .await?
        .max(send::gas_floor(&ready));
    let from = slot.from;
    let nonce = slot.nonce(ctx).await?;
    let gas = tx::limit(ctx, from, to, &data, value).await?;
    let signed = signer::sign(ctx, tx::legacy(ctx, nonce, price, gas, to, value, data)?)?;
    let tip = match send::tip(&ready).filter(|_| with_tip) {
        Some((tip_to, amount)) => {
            let g = tx::limit(ctx, from, tip_to, &Bytes::new(), amount).await?;
            let t = tx::legacy(ctx, nonce + 1, price, g, tip_to, amount, Bytes::new())?;
            Some((signer::sign(ctx, t)?, amount))
        }
        None => None,
    };
    let head = env.heads.fresh(ctx)?;
    let d = send::dispatch(
        ctx,
        &env.stats,
        &ready,
        &signed.raw,
        tip.as_ref().map(|t| &t.0.raw),
        head.number + cfg.bundle_max,
    )
    .await?;
    let method = if ready.iter().all(|r| r.method == SendMethod::Bundle) {
        SendMethod::Bundle
    } else {
        SendMethod::Single
    };
    let channels = d.all();
    let watch = Watch {
        ctx: ctx.clone(),
        heads: env.heads.clone(),
        stats: env.stats.clone(),
        ready,
        cfg,
        hash: signed.hash,
        nonce,
        from,
        price,
        sent_block: head.number,
        channels: channels.clone(),
    };
    let hold = slot.hold();
    let after = After {
        ctx,
        heads: &env.heads,
        stats: &env.stats,
        cfg: &cfg,
        from,
        nonce,
        hash: signed.hash,
    };
    settle::busy(&after, &hold);
    let (task_ctx, task_heads, task_stats) = (ctx.clone(), env.heads.clone(), env.stats.clone());
    let hash = signed.hash;
    let job = tokio::spawn(async move {
        let t = track::resolve(watch).await;
        drop(keep);
        let a = After {
            ctx: &task_ctx,
            heads: &task_heads,
            stats: &task_stats,
            cfg: &cfg,
            from,
            nonce,
            hash,
        };
        settle::settle(&a, &hold, &t).await;
        t
    });
    let t = job.await.map_err(|e| {
        ctx.err(
            ErrorCode::Internal,
            format!("задача трекера {} упала: {e}", signed.hash),
        )
    })?;
    let tip_bnb = tip.and_then(|(_, a)| num::to_decimal(a, num::BNB_DECIMALS));
    if let Some(why) = &t.unresolved {
        let chain = chain_tx(signed.hash, nonce, channels, method, &t, tip_bnb);
        return Err(ctx
            .err(
                ErrorCode::OutcomeUnknown,
                format!("Исход транзакции {} неизвестен ({why}) — очередь кошелька остановлена до ручной проверки", signed.hash),
            )
            .with_tx(chain));
    }
    Ok(Done {
        chain: chain_tx(signed.hash, nonce, channels, method, &t, tip_bnb),
        receipt: t.receipt,
        cancel: t.cancel,
    })
}

/// ChainTx из итога. Не попала и nonce заняла отмена — газ отмены (он потрачен) в gas_used / gas_bnb, блока нет
/// (хеш отмены — в Done.cancel и в журнале; поля в стыке ChainTx нет — запрос на изменение стыка). Ошибок нет.
fn chain_tx(
    hash: B256,
    nonce: u64,
    channels: Vec<String>,
    method: SendMethod,
    t: &Tracked,
    tip_bnb: Option<Decimal>,
) -> ChainTx {
    let r = t.receipt.as_ref().or(t.cancel.as_ref().map(|c| &c.1));
    let own = t.receipt.as_ref();
    ChainTx {
        hash,
        nonce,
        channels,
        method,
        outcome: t.outcome,
        block: own.map(|r| r.block_number.to()),
        gas_used: r.map(|r| r.gas_used.to()),
        gas_bnb: r.and_then(|r| r.gas_bnb()),
        tip_bnb: if t.outcome == TxOutcome::Included || t.outcome == TxOutcome::Reverted {
            tip_bnb
        } else {
            None
        },
    }
}
