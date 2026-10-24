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

use crate::core::types::{ChainTx, SendMethod};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::num;
use super::super::quote::gas::GasPrice;
use super::super::send::settle::{self, After};
use super::super::send::stats::Stats;
use super::super::send::track::{self, Receipt, Watch};
use super::super::send::tx::{self, SendCfg};
use super::super::send::{self};
use super::super::wallet::inflows::{Inflows, SwapGuard};
use super::super::wallet::nonce::Slot;
use super::super::wallet::signer;
use crate::gateway::urgency as priority;

/// Всё для отправки. Дешёво клонируется.
#[derive(Clone)]
pub struct Env {
    pub ctx: Ctx,
    pub heads: Heads,
    pub stats: Stats,
    pub gas: GasPrice,
    /// Ожидаемые поступления в кошелёк и идущие обмены (wallet/inflows.rs).
    pub inflows: Inflows,
    /// Разрешения кошелька для котировки — очищаются при каждой транзакции кошелька (wallet/allow_cache.rs).
    pub allow: super::super::wallet::allow_cache::Allowances,
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
    settle::busy(&after, &hold).await;
    // Транзакция ушла — разрешения кошелька могли измениться (approve, обмен списывает разрешение).
    env.allow.clear();
    let (task_ctx, task_heads, task_stats) = (ctx.clone(), env.heads.clone(), env.stats.clone());
    let hash = signed.hash;
    // Слежение за транзакцией — тем же классом срочности, что и отправка (gateway/urgency.rs: задача tokio класс не наследует).
    let class = priority::current();
    let job = tokio::spawn(priority::scope(class, async move {
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
    }));
    let joined = job.await;
    env.allow.clear();
    let t = joined.map_err(|e| {
        ctx.err(
            ErrorCode::Internal,
            format!("задача трекера {} упала: {e}", signed.hash),
        )
    })?;
    let tip_bnb = tip.and_then(|(_, a)| num::to_decimal(a, num::BNB_DECIMALS));
    if let Some(why) = &t.unresolved {
        let chain = super::chain_tx::chain_tx(signed.hash, nonce, channels, method, &t, tip_bnb);
        return Err(ctx
            .err(
                ErrorCode::OutcomeUnknown,
                format!("Исход транзакции {} неизвестен ({why}) — очередь кошелька остановлена до ручной проверки", signed.hash),
            )
            .with_tx(chain));
    }
    Ok(Done {
        chain: super::chain_tx::chain_tx(signed.hash, nonce, channels, method, &t, tip_bnb),
        receipt: t.receipt,
        cancel: t.cancel,
    })
}
