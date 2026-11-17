//! Файл: venues/bsc_dex/check/memory_proof.rs
//! Что делает: решение 382 (ревью Б1 и ревью 2 Б1) — доказательство, что память пула НЕВЕРНА, а не просто отстаёт.
//! Сравнение памяти с узлом на голове H само по себе ошибку не доказывает (событие блока H может быть в пути). Правило:
//! 1. логи пула за блоки [S, H] одним eth_getLogs (S — блок памяти, Entry.synced_block; фильтр — тот же, что у подписки
//!    пачки: адрес своего контракта, у V4 / Infinity — менеджер и topic1 = PoolId, topic0 — события вида,
//!    pools/group.rs filter, node/subs.rs) и отбор тех, что память НЕ применила (Entry::has_applied);
//! 2. есть неприменённый лог в блоке ≤ H − dex.quoter_memory_lag_blocks — событие пропущено, ошибка доказана (Wrong,
//!    сильная: пропуск мог быть Mint/Burn — карта тиков, её следующий Swap не правит);
//! 3. неприменённые только свежее (> H − отставания) — это отставание (Lag), без отметки;
//! 4. неприменённых нет (память учла все логи пула до H, и её блок не позже H) — память обязана совпасть с узлом на конце
//!    H: сравнение верха и карты тиков в общих словах (exec/quoter_refetch_note.rs differs) с пулом, прочитанным на H
//!    (переданным вызывающим или прочитанным здесь). Различаются — ошибка доказана (Wrong; сильная, если неверна карта
//!    тиков); равны — Right.
//!
//! Пары общего контракта (WooFi, PoolRef::Member) и многомонетного пула (Multi): логи контракта касаются и других пар,
//! а позицию пары ведёт её перечитывание — п. 2 к ним не применяется (неприменённые логи — Lag), п. 4 — как у всех.
//! Не задана dex.quoter_memory_lag_blocks — доказательство не выполняется (Unproven с её именем — видно в журнале).
//! Пределы своего узла (tests/data/node_limits_2026-11-12): eth_getLogs — диапазон до 5000 блоков, состояние — 128
//! последних блоков; больше — ответ узла с ошибкой → Unproven с её текстом (п. 4 читает только голову H).
//! Кто вызывает: exec/quoter_refetch_proof.rs (фоном после перечитывания перед сделкой), check/quoter_memory_fix.rs.
//! От чего зависит: node/rpc.rs, node/call.rs, node/subs.rs, pools/events.rs (topics), pools/load.rs, pools/registry.rs,
//! exec/quoter_refetch_note.rs, check/quoter_block.rs (MEMORY_LAG).
//! Настройки: dex.quoter_memory_lag_blocks [bsc_dex]; через pools/load.rs — настройки чтения вида пула.
//!
//! eth_getLogs(filter: address, topics, fromBlock, toBlock) — execution-apis
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth (eth_getLogs).

use alloy::rpc::types::Log;
use serde_json::Value;

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::exec::quoter_refetch_note;
use super::super::node::{call, rpc, subs};
use super::super::pools::registry::{Entry, Registry};
use super::super::pools::{events, load};
use super::super::tokens::Tokens;
use super::quoter_block::MEMORY_LAG;

/// Итог доказательства.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proof {
    /// Память неверна: что не так и «сильная» ли ошибка (пропущенное событие или карта тиков — следующий Swap её не
    /// поправит).
    Wrong { note: String, strong: bool },
    /// Память отстаёт (события в пути) — причина; не ошибка.
    Lag(String),
    /// Память равна узлу на H.
    Right,
    /// Доказать нельзя — причина.
    Unproven(String),
}

/// Неприменённые памятью логи (чистая логика): позиции (блок, logIndex) логов узла, которые память не применила.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unapplied {
    /// Нет ни одного.
    None,
    /// Самый ранний — старше головы больше допуска отставания: событие пропущено.
    Missed((u64, u64)),
    /// Только свежие (в пути): сколько и самый ранний.
    Fresh(usize, (u64, u64)),
}

/// Разбор логов узла против памяти (чистая логика, см. шапку п. 2–3). Принимает: позиции логов (блок, logIndex;
/// отменённые реоргом уже отброшены), «применено ли» по позиции, голову, допуск отставания (блоков). Ошибок нет.
pub fn unapplied(
    logs: &[(u64, u64)],
    applied: impl Fn(u64, u64) -> bool,
    head: u64,
    lag: u64,
) -> Unapplied {
    let mut left: Vec<(u64, u64)> = logs
        .iter()
        .copied()
        .filter(|(b, i)| !applied(*b, *i))
        .collect();
    left.sort_unstable();
    match left.first() {
        None => Unapplied::None,
        Some(first) if first.0 <= head.saturating_sub(lag) => Unapplied::Missed(*first),
        Some(first) => Unapplied::Fresh(left.len(), *first),
    }
}

/// Позиции логов ответа (без отменённых реоргом и без позиции). Ошибок нет.
pub fn positions(logs: &[Log]) -> Vec<(u64, u64)> {
    logs.iter()
        .filter(|l| !l.removed)
        .filter_map(|l| Some((l.block_number?, l.log_index?)))
        .collect()
}

/// Фильтр логов пула — как у подписки пачки (pools/group.rs filter) — за блоки [from, to]. Ошибок нет.
pub fn filter(e: &Entry, from: u64, to: u64) -> Value {
    let topic0 = events::topics(e.family);
    let mut f = match &e.pool.reference {
        PoolRef::Singleton { manager, id } => subs::by_pool_id(*manager, &[*id], &topic0),
        r => subs::by_address(&[r.address()], &topic0),
    };
    f["fromBlock"] = Value::String(call::block_tag(from));
    f["toBlock"] = Value::String(call::block_tag(to));
    f
}

/// Логи пула за блоки [from, to] (eth_getLogs). Ошибки: узел (в том числе предел диапазона), ответ не по виду.
async fn logs_of(ctx: &Ctx, e: &Entry, from: u64, to: u64) -> GatewayResult<Vec<Log>> {
    rpc::call(ctx, "eth_getLogs", serde_json::json!([filter(e, from, to)])).await
}

/// Доказательство (см. шапку). Принимает: окружение, токены, реестр, снимок записи памяти, голову H, пул, прочитанный
/// на H (None — прочитать здесь). Возвращает: Proof. Ошибок нет (сбой узла — Unproven с причиной).
pub async fn prove(
    ctx: &Ctx,
    (tokens, reg): (&Tokens, &Registry),
    mem: &Entry,
    head: u64,
    at_head: Option<&Entry>,
) -> Proof {
    let Ok(lag) = ctx.int(MEMORY_LAG) else {
        return Proof::Unproven(format!(
            "не задана {MEMORY_LAG} — доказательство ошибки памяти не выполняется"
        ));
    };
    let s = mem.synced_block;
    if s > head {
        return Proof::Lag(format!(
            "память (блок {s}) новее головы {head} — сравнивать не с чем"
        ));
    }
    let logs = match logs_of(ctx, mem, s, head).await {
        Ok(l) => positions(&l),
        Err(e) => return Proof::Unproven(format!("eth_getLogs блоков {s}..{head}: {}", e.message)),
    };
    let own = matches!(
        mem.pool.reference,
        PoolRef::Contract(_) | PoolRef::Singleton { .. }
    );
    match unapplied(&logs, |b, i| mem.has_applied(b, i), head, lag) {
        Unapplied::Missed((b, i)) if own => {
            return Proof::Wrong {
                note: format!(
                    "{} {}: событие пула блок {b}, logIndex {i} есть у узла, память его не применила (память — блок \
                     {s}{}, голова {head}, допуск отставания {MEMORY_LAG} = {lag})",
                    mem.family.title,
                    mem.ticker,
                    mem.synced_log
                        .map_or(String::new(), |k| format!(", logIndex {k}"))
                ),
                strong: true,
            };
        }
        Unapplied::Missed((b, i)) | Unapplied::Fresh(_, (b, i)) => {
            return Proof::Lag(format!(
                "событий пула, не применённых памятью: {}, самое раннее — блок {b}, logIndex {i}",
                logs.iter()
                    .filter(|(x, y)| !mem.has_applied(*x, *y))
                    .count()
            ));
        }
        Unapplied::None => {}
    }
    let read;
    let node = match at_head {
        Some(n) => n,
        None => match load::refresh(ctx, tokens, reg, mem, head).await {
            Ok(n) => {
                read = n;
                &read
            }
            Err(e) => {
                return Proof::Unproven(format!("чтение пула на блоке {head}: {}", e.message));
            }
        },
    };
    if quoter_refetch_note::differs(mem, node) {
        Proof::Wrong {
            note: quoter_refetch_note::note(mem, node, head),
            strong: quoter_refetch_note::ticks_differ(mem, node),
        }
    } else {
        Proof::Right
    }
}
