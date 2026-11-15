//! Файл: venues/bsc_dex/exec/approve_seen.rs
//! Что делает: решение 303 (задача t134; живой бот 15.11.2026, сделка 20261115213829104FOREST: через 7 мс после
//! dex.rebuilt_after_approve проба перед отправкой откатилась Permit2 AllowanceExpired со сроком 12.11). После того как
//! разрешения (approve) легли в блок N по квитанции своего узла (exec/swap_send.rs send_approvals), пересборка обмена
//! читает разрешения на «latest» (wallet/approve.rs — уже видит approve), а проба (exec/precheck_sim.rs) и предел блока
//! бандла (exec/txs.rs) идут от головы опроса блоков (node/heads.rs, решение 57 — опрос раз в bsc.blocks_poll_ms) и её
//! кэша времени (Heads::at, решение 89 — при занятом запросе отдаёт ПОСЛЕДНЕЕ известное время любого блока). Голова
//! опроса могла ещё не дойти до N — проба шла по состоянию без approve. Здесь: ждём, пока голова опроса дойдёт до N
//! (не дольше feed.stale_after_ms — дольше голова и так «устарела»), и кладём время головы ≥ N в кэш — следующая
//! sim_base::head (проба, срок транзакции) и heads.fresh (предел блока бандла) дают блок не старше блока approve.
//! Кто вызывает: exec/swap_send.rs (send_approvals — прямой, умный путь, исполнитель, цикл через исполнитель).
//! От чего зависит: node/heads.rs (watch, fresh, block_time, remember_time), node/head_age.rs (fetch), ctx.rs.
//! Настройки: feed.stale_after_ms [bsc_dex] (предел ожидания; новых нет).

use std::time::Duration;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::head_age;
use super::super::node::heads::Heads;

/// Ждёт, пока голова опроса блоков дойдёт до блока `block` (блок квитанции последнего approve), и обновляет кэш
/// времени головы, если в нём блок старше. Принимает: окружение, головы, номер блока approve.
/// Возвращает: номер головы, от которой дальше пойдут проба и отправка (≥ block).
/// Ошибки: NotConfigured (feed.stale_after_ms), StaleData — голова не дошла до блока approve за feed.stale_after_ms
/// (проба по состоянию без approve не идёт); Internal — опрос блоков остановлен; узел (время блока).
pub async fn wait(ctx: &Ctx, heads: &Heads, block: u64) -> GatewayResult<u64> {
    let limit = ctx.int("feed.stale_after_ms")?;
    let mut rx = heads.watch();
    let reached = rx.wait_for(|h| h.as_ref().is_some_and(|h| h.number >= block));
    // Ref приёмника не держим через await: только «дошла / опрос остановлен».
    let waited = tokio::time::timeout(Duration::from_millis(limit), reached)
        .await
        .map(|r| r.is_ok());
    match waited {
        Ok(true) => {}
        Ok(false) => {
            return Err(ctx.err(
                ErrorCode::Internal,
                "опрос блоков остановлен — проба обмена после разрешения не идёт",
            ));
        }
        Err(_) => {
            let seen = heads.latest().map(|h| h.number);
            let why = format!(
                "разрешение (approve) в блоке {block}, а голова опроса блоков за feed.stale_after_ms ({limit} мс) \
                 дошла только до {} — проба и обмен по состоянию без разрешения не идут (решение 303)",
                seen.map_or("—".to_string(), |n| n.to_string())
            );
            return Err(ctx.err(ErrorCode::StaleData, why));
        }
    }
    let head = heads.fresh(ctx)?;
    // Кэш времени отстаёт от блока approve — Heads::at при занятом запросе отдал бы старый блок (решение 89).
    if heads.block_time().is_none_or(|(n, _)| n < block) {
        let t = head_age::fetch(ctx, head.number, true).await?;
        heads.remember_time(head.number, t);
    }
    Ok(head.number)
}
