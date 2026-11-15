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
//! Журнал dex.approve_seen (дополнение 303, для разбора случаев вроде SIREN 15.11 20:49:06): по строке на каждый
//! approve — хеш, блок квитанции, голова опроса при квитанции; у последнего — сколько мс ждали головы ≥ N и какая
//! голова получена (или почему не дождались). Пишется в отдельной задаче — запись в базу не задерживает обмен.
//! Кто вызывает: exec/swap_send.rs (send_approvals — прямой, умный путь, исполнитель, цикл через исполнитель).
//! От чего зависит: node/heads.rs (watch, fresh, latest, block_time, remember_time), node/head_age.rs (fetch), ctx.rs.
//! Настройки: feed.stale_after_ms [bsc_dex] (предел ожидания; новых нет).

use std::time::{Duration, Instant};

use alloy_primitives::B256;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::head_age;
use super::super::node::heads::Heads;

/// Одно разрешение в блоке: хеш, блок квитанции, голова опроса блоков в момент квитанции (None — головы не было).
pub struct Seen {
    pub hash: B256,
    pub block: Option<u64>,
    pub head: Option<u64>,
}

/// Ждёт голову опроса ≥ блока последнего approve и пишет строки журнала dex.approve_seen. Принимает: окружение,
/// головы, разрешения по порядку (пусто — ничего). Ошибки: как у wait.
pub async fn settle(ctx: &Ctx, heads: &Heads, seen: &[Seen]) -> GatewayResult<()> {
    let Some(block) = seen.iter().filter_map(|s| s.block).max() else {
        return Ok(());
    };
    let started = Instant::now();
    let res = wait(ctx, heads, block).await;
    let ms = started.elapsed().as_millis();
    let tail = match &res {
        Ok(h) => format!("ждали {ms} мс до головы ≥ {block} (голова {h})"),
        Err(e) => format!("за {ms} мс голова ≥ {block} не получена: {}", e.message),
    };
    let lines: Vec<String> = seen
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let last = i + 1 == seen.len();
            format!(
                "approve {} в блоке {}, голова опроса при квитанции {}{}",
                s.hash,
                opt(s.block),
                opt(s.head),
                if last {
                    format!("; {tail}")
                } else {
                    String::new()
                }
            )
        })
        .collect();
    let c = ctx.clone();
    tokio::spawn(async move {
        for l in lines {
            c.log(Level::Info, "dex.approve_seen", l, None).await;
        }
    });
    res.map(|_| ())
}

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
                opt(seen)
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

/// Число или «—». Ошибок нет.
fn opt(v: Option<u64>) -> String {
    v.map_or("—".to_string(), |n| n.to_string())
}
