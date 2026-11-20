//! Файл: venues/bsc_dex/check/memory_proof_rules.rs
//! Что делает: решение 382 (ревью 2–3) — чистые правила доказательства «память пула неверна» (без узла; узел —
//! check/memory_proof.rs):
//! - виды «каждым блоком» (pools/per_block.rs needs: Lista, WooFi, Infinity Stable, Curve NG, Nomi, Metric, iZi/Biswap
//!   V3, Kyber DMM, SmarDex, Balancer V2, LB) меняют состояние без логов своего адреса (оракул, время блока, Vault) —
//!   «логов нет» у них не значит «состояние то же»: сравнение с узлом — только на блоке самой памяти (S = H, память
//!   прочитана целиком), иначе отставание (per_block_gate, ревью 3 Б1);
//! - неприменённые памятью логи (unapplied): пропуск (старше головы больше допуска отставания) или свежие;
//! - чьи логи доказывают пропуск (own): свой контракт, V4 / Infinity (менеджер + PoolId) и пары многомонетного пула
//!   (Multi: по коду pools/listen_site.rs каждая глубокая пара перечитывается по каждому логу контракта, кроме уже
//!   учтённого — её позиция покрывает логи контракта, ревью 3 Н2); пары общего контракта WooFi (Member) — вид «каждым
//!   блоком» (WooPP), их ведёт per_block_gate;
//! - узкое окно свежих логов (narrow_from) — когда логи за весь [S, H] узел не отдал (предел диапазона, ревью 3 Б2):
//!   свежие неприменённые в последних dex.quoter_memory_lag_blocks блоках — отставание; нет — сравнение на H (у
//!   событийных видов без событий состояние то же, пропущенное раньше окна событие и есть расхождение);
//! - фильтр логов пула — как у подписки пачки (pools/group.rs filter, node/subs.rs); Ekubo (ревью 447): PoolId не в
//!   topics — фильтр по адресу ядра без topics, логи пула отбирает mine по PoolId из данных (pools/ekubo_route.rs);
//!   пул Ekubo с TWAMM меняет состояние со временем без логов — как вид «каждым блоком» (per_block_gate, own).
//!
//! Кто вызывает: check/memory_proof.rs; тесты (tests/bsc_dex_quoter_refetch_382.rs).
//! От чего зависит: pools/per_block.rs (needs), pools/events.rs (topics), node/subs.rs, node/call.rs, pools/registry.rs.
//! Настройки: нет (допуск отставания передаёт вызывающий — dex.quoter_memory_lag_blocks).

use alloy::rpc::types::Log;
use serde_json::Value;

use crate::core::types::{PoolRef, PoolState};

use super::super::node::{call, subs};
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;
use super::super::pools::{ekubo_route, events, per_block};

/// Неприменённые памятью логи узла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unapplied {
    /// Нет ни одного.
    None,
    /// Самый ранний — старше головы больше допуска отставания: событие пропущено (позиция).
    Missed((u64, u64)),
    /// Только свежие (в пути): сколько и самый ранний.
    Fresh(usize, (u64, u64)),
}

/// Разбор логов узла против памяти. Принимает: позиции логов (блок, logIndex), «применено ли» по позиции, голову,
/// допуск отставания (блоков). Ошибок нет.
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

/// Фильтр логов пула — как у подписки пачки — за блоки [from, to]. Ошибок нет.
pub fn filter(e: &Entry, from: u64, to: u64) -> Value {
    let topic0 = events::topics(e.family);
    let mut f = match &e.pool.reference {
        // Ревью 447: у ядра Ekubo PoolId в данных — все логи ядра, отбор — mine.
        PoolRef::Singleton { manager, .. } if e.family.proto == Proto::Ekubo => {
            serde_json::json!({ "address": manager })
        }
        PoolRef::Singleton { manager, id } => subs::by_pool_id(*manager, &[*id], &topic0),
        r => subs::by_address(&[r.address()], &topic0),
    };
    f["fromBlock"] = Value::String(call::block_tag(from));
    f["toBlock"] = Value::String(call::block_tag(to));
    f
}

/// Лог узла относится к пулу записи: у Ekubo — PoolId из данных лога (ответ фильтра — все логи ядра); у прочих —
/// любой (фильтр уже по пулу). Ошибок нет.
pub fn mine(e: &Entry, log: &Log) -> bool {
    match &e.pool.reference {
        PoolRef::Singleton { id, .. } if e.family.proto == Proto::Ekubo => {
            ekubo_route::pool_id(log) == Some(*id)
        }
        _ => true,
    }
}

/// Пул Ekubo с TWAMM: состояние расширения идёт со временем без логов (виртуальные ордера). Ошибок нет.
fn time_driven(e: &Entry) -> bool {
    matches!(&e.pool.state, PoolState::Ekubo(d) if d.twamm.is_some())
}

/// Вид «каждым блоком»: сравнение только на блоке памяти. Принимает: запись памяти, голову. Возвращает: причину
/// «отставание» (Some) — сравнивать нельзя; None — можно (не такой вид или память прочитана целиком на H). Ошибок нет.
pub fn per_block_gate(mem: &Entry, head: u64) -> Option<String> {
    let timed = per_block::needs(mem.family) || time_driven(mem);
    if !timed || (mem.synced_block == head && mem.synced_log.is_none()) {
        return None;
    }
    Some(format!(
        "вид пула меняет состояние каждым блоком без событий пула — сравнение только на блоке памяти (память — блок \
         {}, голова {head})",
        mem.synced_block
    ))
}

/// Доказывают ли неприменённые логи этого пула пропуск события (см. шапку). Ошибок нет.
pub fn own(e: &Entry) -> bool {
    !per_block::needs(e.family)
        && !time_driven(e)
        && matches!(
            e.pool.reference,
            PoolRef::Contract(_) | PoolRef::Singleton { .. } | PoolRef::Multi { .. }
        )
}

/// Начало узкого окна свежих логов: max(S, H − отставание). Ошибок нет.
pub fn narrow_from(s: u64, head: u64, lag: u64) -> u64 {
    s.max(head.saturating_sub(lag))
}
