//! Файл: venues/bsc_dex/exec/txs.rs
//! Что делает: одна транзакция кошелька от начала до итога: каналы (send/mod.rs), цена газа (eth_gasPrice + надбавка,
//! не ниже минимума выбранных каналов), лимит газа (eth_estimateGas — заодно проверка, что вызов не откатится, —
//! плюс запас), подпись (EIP-155, legacy), чаевые (только по просьбе вызывающего — один раз на сделку, к транзакции
//! обмена; и только один канал и бандл), отправка, ожидание итога своим узлом до занятия nonce в блоке.
//! Ожидание идёт в отдельной задаче, которая держит место в очереди кошелька (wallet/nonce.rs, Hold): даже если
//! вызывающий прерван, следующая транзакция кошелька не возьмёт nonce, пока этот не занят (решение 45). Задача трекера
//! упала (паника) или отменена — очередь ОСТАНОВЛЕНА (send/settle.rs guarded / joined), вызывающий получает «исход
//! неизвестен» с «чем найти», как при Pending (решение 231): исход выясняет ядро. В «чем найти» — и хеши отправленных
//! трекером отмен (журнал отмен send/track_util.rs CancelLog): позднее выяснение проверяет и их квитанции.
//! Кто вызывает: exec/trade.rs, wallet/transfer.rs.
//! От чего зависит: exec/chain_tx.rs (факт; газ перевода чаевых — решение 236, дополнение п. 6), exec/swap_ctx.rs (seed — «чем найти» с чаевыми, решение 231), send/*, quote/gas.rs, wallet/signer.rs, wallet/nonce.rs, wallet/inflows.rs, node/heads.rs, num.rs.
//! Настройки: send.* через send/tx.rs (SendCfg — читается до отправки), channel.*.
//!
//! eth_estimateGas, legacy-транзакция EIP-155 — ссылки в send/tx.rs; трекер и отмена — send/track.rs, send/cancel.rs.

use alloy_primitives::{B256, Bytes, TxKind, U256};

use crate::core::types::{ChainTx, SendMethod, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::num;
use super::super::quote::gas::GasPrice;
use super::super::send::settle::{self, After};
use super::super::send::stats::Stats;
use super::super::send::track::{self, Receipt, Tracked, Watch};
use super::super::send::track_util::{CancelLog, cancels_of};
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
    /// Чаевые, отправленные вместе с транзакцией (BNB; None — не было). В chain.tip_bnb они попадают только при исходе
    /// «в блоке» / «откат»; здесь — всегда, для «чем найти» (решение 231, дополнение: exec/swap_ctx.rs).
    pub tip_bnb: Option<rust_decimal::Decimal>,
    /// Хеши всех отправленных отмен (send/track.rs) — для «чем найти» при «вытеснена» (exec/result.rs,
    /// exec/swap_send.rs): позднее выяснение (resolve_single.rs) проверяет и их квитанции.
    pub cancels: Vec<B256>,
}

/// Отправляет вызов и ждёт, пока его nonce не занят в блоке. Принимает: окружение, место в очереди кошелька, адрес
/// (или TxKind::Create — развёртывание исполнителя, решение 122),
/// данные, сумму BNB (wei), чаевые (true — только для транзакции обмена), отметку «идёт обмен» (wallet/inflows.rs —
/// снимается вместе с местом в очереди, когда nonce занят).
/// Возвращает: Done (итог может быть «откат» / «не попала» / «вытеснена» — решает вызывающий).
/// Ошибки (транзакция не ушла ни в один канал): настройки, нет каналов, eth_estimateGas откатился (текст узла),
/// все каналы ответили отказом. После отправки: OutcomeUnknown с фактом транзакции и «чем найти» — исход не установлен
/// (Pending трекера или слежение прервано паникой / отменой задачи: очередь кошелька остановлена, send/settle.rs).
pub async fn send(
    env: &Env,
    slot: &Slot,
    to: impl Into<TxKind>,
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
    let to: TxKind = to.into();
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
    let cancel_log = CancelLog::default();
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
        cancels: cancel_log.clone(),
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
    // Удержание вызывающего: если задача трекера упадёт мимо своей защиты или будет отменена, очередь остановит он
    // (send/settle.rs joined), пока место ещё держится.
    let hold_out = hold.clone();
    // Слежение за транзакцией — тем же классом срочности, что и отправка (gateway/urgency.rs: задача tokio класс не наследует).
    let class = priority::current();
    let job = tokio::spawn(priority::scope(class, async move {
        let a = After {
            ctx: &task_ctx,
            heads: &task_heads,
            stats: &task_stats,
            cfg: &cfg,
            from,
            nonce,
            hash,
        };
        // Паника внутри слежения — очередь остановлена здесь же, удержанием задачи (send/settle.rs guarded): даже если
        // вызывающий уже прерван, следующая транзакция поверх не уйдёт.
        settle::guarded(&a, &hold, async {
            let t = track::resolve(watch).await;
            drop(keep);
            settle::settle(&a, &hold, &t).await;
            t
        })
        .await
    }));
    let joined = settle::joined(&after, &hold_out, job).await;
    env.allow.clear();
    // Слежение прервано (паника, отмена) — исход не установлен, очередь уже остановлена: дальше как при Pending —
    // «исход неизвестен» с «чем найти» (хеш, nonce, чаевые), исход выясняет ядро (решение 231).
    let t = joined.unwrap_or_else(|why| Tracked::bare(TxOutcome::Pending, Some(why)));
    let tip_hash = tip.as_ref().map(|(s, _)| s.hash);
    let tip_bnb = tip.and_then(|(_, a)| num::to_decimal(a, num::BNB_DECIMALS));
    let cancels = cancels_of(&cancel_log);
    let mut chain = super::chain_tx::chain_tx(signed.hash, nonce, channels, method, &t, tip_bnb);
    // Решение 236, дополнение п. 6: чаевые — отдельная транзакция (nonce + 1); её хеш — в факт (позднее выяснение).
    chain.tip_hash = tip_hash;
    if let Some(why) = &t.unresolved {
        // Решение 231 (дополнение): чаевые — в «чем найти» (в chain.tip_bnb их нет), чтобы итог свопа по квитанции совпал
        // с обычным путём (exec/swap_rebuild.rs); хеши отмен — тоже: nonce могла занять отмена (resolve_single.rs).
        let seed = super::swap_ctx::with_cancels(super::swap_ctx::seed(&chain, tip_bnb), &cancels);
        return Err(ctx
            .err(
                ErrorCode::OutcomeUnknown,
                format!("Исход транзакции {} неизвестен ({why}) — очередь кошелька остановлена до ручной проверки", signed.hash),
            )
            .with_tx(chain)
            .pending(seed));
    }
    Ok(Done {
        // Чаевые в блоке — газ их перевода по квитанции (chain_tx.rs tip_gas), в затраты сделки (core/deal_gas.rs).
        chain: super::chain_tx::tip_gas(ctx, chain).await,
        receipt: t.receipt,
        cancel: t.cancel,
        tip_bnb,
        cancels,
    })
}
