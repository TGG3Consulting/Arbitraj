//! Файл: venues/bsc_dex/exec/bundle_send.rs
//! Что делает: отправка собранного бандла свопов (exec/bundle.rs, решение 84). Вызывается боевым путём графа
//! (exec/bundle_trade.rs) с местом в очереди кошелька Slot. Шаги: (1) симуляция всего упорядоченного бандла
//! eth_simulateV1 на своём узле (check/simulate.rs) — видит эффект ноги N на ногу N+1 (межтранзакционная передача) и
//! валидирует порядок; откат любого шага → не отправляем; попутно газ и кумулятивные базы BALANCE_CHECK (exec/bundle_sim.rs);
//! (2) проверка «кто меняет» (решение 59) по событиям симуляции — чужой пул исключается; (3) цена газа — как у одиночной;
//! (4) подпись по очереди (exec/bundle_sim.rs); (5) отправка одним бандлом через бандл-каналы (send/mod.rs dispatch_slice →
//! eth_sendBundle, revertingTxHashes пуст = всё-или-ничего, maxBlockNumber = голова + send.bundle_max_blocks);
//! (6) ожидание блока и settle очереди — в ОТДЕЛЬНОЙ задаче, держащей место в очереди (Hold), как у одиночной
//! (exec/txs.rs, решение 45): даже если вызывающий прерван в середине ожидания, место держится до установленного итога.
//! Приватный all-or-nothing бандл истекает по maxBlockNumber (газ не тратится, решение 40): «не попал в блок» —
//! УСТАНОВЛЕННЫЙ исход с НЕзанятым nonce → очередь свободна сразу (settle::settle_bundle), следующая транзакция берёт
//! тот же первый nonce; очередь НЕ останавливается по таймауту (ревью #82). Отмен у бандла нет.
//! Классику/одиночную отправку не трогает.
//! Кто вызывает: exec/bundle_trade.rs; тесты.
//! От чего зависит: exec/bundle.rs, exec/bundle_sim.rs, exec/txs.rs (Env), check/{simulate,emitter}.rs,
//! send/{mod,receipt,settle,track_util}.rs, wallet/nonce.rs, wallet/signer.rs, node/heads.rs, gateway/urgency.rs.
//! Настройки: send.gas_limit_margin_pct, send.bundle_max_blocks, send.queue_max_wait_s, feed.stale_after_ms [bsc_dex]
//! (через SendCfg); цена газа — через quote/gas.rs; каналы — channel.* (bundle).

use std::time::Instant;

use alloy_primitives::{B256, Bytes};

use crate::core::types::TxOutcome;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::urgency as priority;

use super::super::check::emitter;
use super::super::check::simulate::{self, SimLog};
use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::pools::registry::Entry;
use super::super::send::receipt::{self, Receipt};
use super::super::send::settle::{self, After};
use super::super::send::track_util::{self, Quiet, Tracked};
use super::super::send::tx::SendCfg;
use super::super::send::{self};
use super::super::wallet::nonce::Slot;
use super::super::wallet::signer;
use super::bundle::Bundle;
use super::bundle_sim::{build_sim, collect, sign_txs};
use super::txs::Env;

/// Итог отправки бандла.
pub struct Sent {
    /// Хеши транзакций бандла по порядку.
    pub hashes: Vec<B256>,
    /// Каналы, где бандл может быть (приняли или не ответили ясно).
    pub channels: Vec<String>,
    /// Попадание в блок по своему узлу.
    pub tracked: Tracked,
    /// Квитанции всех транзакций бандла (только если попал в блок) — для факта сделки (exec/bundle_trade.rs).
    pub receipts: Vec<Receipt>,
}

/// Отправляет собранный бандл, занимая очередь кошелька местом `slot` (nonce в бандле уже проставлены от slot.nonce).
/// Шаги: симуляция всего бандла (газ + кумулятивные базы BALANCE_CHECK + проверка «кто меняет», решение 59) → подпись
/// по очереди → очередь «ждёт итога» (settle::busy) → отправка одним eth_sendBundle через бандл-каналы → ожидание блока
/// и settle очереди в отдельной задаче-трекере (держит Hold). Принимает: окружение, место в очереди, бандл, пулы
/// маршрута (для решения 59). Возвращает: итог с квитанциями. Ошибки: NotConfigured (настройки/каналы), NoRoute/StaleData
/// (симуляция откатилась / чужой пул), ChannelRejected, узел, Internal (подпись / задача трекера упала).
pub async fn send(
    env: &Env,
    slot: &Slot,
    bundle: &Bundle,
    entries: &[&Entry],
) -> GatewayResult<Sent> {
    let ctx = &env.ctx;
    let cfg = SendCfg::read(ctx)?;
    let ready = send::select(ctx, &env.stats)?;
    let wallet = slot.from;
    let price = env
        .gas
        .wei(ctx, &env.heads)
        .await?
        .max(send::gas_floor(&ready));
    let (at, _now) = env.heads.at(ctx).await?;
    let margin = ctx.dec("send.gas_limit_margin_pct")?;
    // Одна симуляция всего упорядоченного бандла: газ, кумулятивные базы BALANCE_CHECK и события обмена.
    let plan = build_sim(bundle, wallet);
    let res = simulate::run_valued(ctx, &plan.calls, &plan.values, at).await?;
    let (limits, bases) = collect(ctx, &res, &plan, margin)?;
    // Решение 59 («кто меняет»): событие обмена — только от пулов маршрута; чужой пул исключается, сделки нет.
    let logs: Vec<SimLog> = res.iter().flat_map(|r| r.logs.clone()).collect();
    emitter::check(ctx, entries, &logs)
        .await
        .map_err(|w| ctx.err(ErrorCode::NoRoute, w))?;
    let s = signer::signer(ctx)?;
    let signed = sign_txs(ctx, &s, bundle, price, &limits, &bases)?;
    let head = env.heads.fresh(ctx)?;
    let max_block = head.number + cfg.bundle_max;
    let raw: Vec<Bytes> = signed.iter().map(|t| t.raw.clone()).collect();
    let dispatched = send::dispatch_slice(ctx, &env.stats, &ready, &raw, max_block).await?;
    let hashes: Vec<B256> = signed.iter().map(|t| t.hash).collect();
    // Очередь кошелька: ждёт итога по последнему nonce бандла (решение 45). Отмен нет — приватный бандл истекает.
    let last_nonce = bundle.first_nonce + (signed.len() as u64) - 1;
    let repr = *hashes.last().unwrap_or(&B256::ZERO);
    let hold = slot.hold();
    let after = After {
        ctx,
        heads: &env.heads,
        stats: &env.stats,
        cfg: &cfg,
        from: wallet,
        nonce: last_nonce,
        hash: repr,
    };
    settle::busy(&after, &hold).await;
    // Ожидание итога и settle — в отдельной задаче, держащей Hold (как у одиночной, exec/txs.rs, решение 45): место в
    // очереди не освобождается, пока итог не установлен, даже если вызывающий прерван.
    let (task_ctx, task_heads, task_stats) = (ctx.clone(), env.heads.clone(), env.stats.clone());
    let task_hashes = hashes.clone();
    let class = priority::current();
    let job = tokio::spawn(priority::scope(class, async move {
        let tracked = confirm(&task_ctx, &task_heads, &cfg, &task_hashes, max_block).await;
        let receipts = if tracked.outcome == TxOutcome::Included {
            all_receipts(&task_ctx, &task_hashes).await
        } else {
            Vec::new()
        };
        let a = After {
            ctx: &task_ctx,
            heads: &task_heads,
            stats: &task_stats,
            cfg: &cfg,
            from: wallet,
            nonce: last_nonce,
            hash: repr,
        };
        settle::settle_bundle(&a, &hold, &tracked).await;
        (tracked, receipts)
    }));
    let (tracked, receipts) = job.await.map_err(|e| {
        ctx.err(
            ErrorCode::Internal,
            format!("задача трекера бандла {repr} упала: {e}"),
        )
    })?;
    Ok(Sent {
        hashes,
        channels: dispatched.all(),
        tracked,
        receipts,
    })
}

/// Квитанции всех транзакций бандла (бандл атомарен — при попадании в блок есть все). Пропавшие пропускаются (решает
/// вызывающий по числу). Ошибок нет — сбой узла по хешу = нет квитанции.
async fn all_receipts(ctx: &Ctx, hashes: &[B256]) -> Vec<Receipt> {
    let mut out = Vec::with_capacity(hashes.len());
    for h in hashes {
        if let Ok(Some((_, r))) = receipt::find(ctx, std::slice::from_ref(h)).await {
            out.push(r);
        }
    }
    out
}

/// Ждёт попадания бандла в блок своим узлом (решение 42) без отмен: приватный бандл с maxBlockNumber истекает сам —
/// прошёл предел блока, квитанции нет → NotIncluded (газ не потрачен, решение 40). Принимает: хеши бандла, предел блока.
/// Возвращает: итог. Ошибок нет — сбои узла повторяются; итог не установлен за send.queue_max_wait_s → Pending.
async fn confirm(
    ctx: &Ctx,
    heads: &Heads,
    cfg: &SendCfg,
    hashes: &[B256],
    max_block: u64,
) -> Tracked {
    let started = Instant::now();
    let mut rx = track_util::heads_rx(heads);
    let repr = hashes.first().copied().unwrap_or_default();
    let mut quiet = Quiet::new(ctx, repr);
    loop {
        if started.elapsed() > cfg.queue_max_wait {
            let why = format!(
                "итог бандла не установлен за send.queue_max_wait_s: {}",
                quiet.reason()
            );
            return Tracked::bare(TxOutcome::Pending, Some(why));
        }
        let Some(head) = track_util::next_head(&mut rx, cfg, &mut quiet).await else {
            continue;
        };
        match receipt::find(ctx, hashes).await {
            Ok(Some((_, r))) => {
                let ok = r.status == alloy_primitives::U64::from(1u8);
                return Tracked {
                    outcome: if ok {
                        TxOutcome::Included
                    } else {
                        TxOutcome::Reverted
                    },
                    receipt: Some(r),
                    cancel: None,
                    unresolved: None,
                };
            }
            Ok(None) => quiet.ok("receipt").await,
            Err(e) => {
                quiet
                    .warn(
                        "receipt",
                        format!("квитанция бандла не прочитана: {}", e.message),
                    )
                    .await;
                continue;
            }
        }
        if head > max_block {
            return Tracked::bare(TxOutcome::NotIncluded, None);
        }
    }
}
