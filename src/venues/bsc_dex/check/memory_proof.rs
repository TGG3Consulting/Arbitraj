//! Файл: venues/bsc_dex/check/memory_proof.rs
//! Что делает: решение 382, ревью Б1 — доказательство, что память пула НЕВЕРНА, а не просто отстаёт. Сравнение памяти с
//! узлом на «чужом» блоке (голова H) ошибку не доказывает: память на H−1, а Swap блока H ещё в пути — это отставание.
//! Доказательство — на блоке САМОЙ памяти S (Entry.synced_block):
//! 1. память посреди блока (synced_log = Some(k)) — eth_getLogs пула за блок S: есть логи с logIndex > k — память
//!    посреди действия, состояние узла на конце S с ней сравнивать нельзя → «не доказано»; нет — память учла все логи
//!    пула блока S и обязана совпасть с узлом на конце S (пул без своего адреса — PoolRef не Contract — «не доказано»);
//! 2. пул читается на S тем же загрузчиком (pools/load.rs refresh; в реестр не пишется), сравнение — верх состояния и
//!    карта тиков в общих словах (exec/quoter_refetch_note.rs differs). Различаются — память неверна (Wrong).
//!
//! Состояние на номер блока у своего узла — только последние 128 блоков (tests/data/node_limits_2026-11-12): старее —
//! «не доказано». eth_getLogs — до 5000 блоков (там же), здесь — один блок.
//! Кто вызывает: exec/quoter_refetch.rs (фоновая задача после перечитывания перед сделкой), check/quoter_memory_fix.rs.
//! От чего зависит: node/rpc.rs, node/call.rs (block_tag), pools/load.rs, exec/quoter_refetch_note.rs.
//! Настройки: через pools/load.rs — настройки чтения вида пула.
//!
//! eth_getLogs(filter: address, fromBlock, toBlock) — execution-apis
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth (eth_getLogs).

use alloy::rpc::types::Log;
use alloy_primitives::Address;
use serde_json::json;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::exec::quoter_refetch_note;
use super::super::node::{call, rpc};
use super::super::pools::load;
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;

/// Итог доказательства.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proof {
    /// Память не равна узлу на своём же блоке: строка «чем отличается» и испорчена ли карта тиков (такая ошибка
    /// переживает следующие Swap — их событие верх поправит, карту нет).
    Wrong { note: String, ticks: bool },
    /// Память равна узлу на своём блоке — расхождение с головой было отставанием.
    Right,
    /// Доказать нельзя (причина) — без отметки.
    Unproven(String),
}

/// Есть ли среди логов пула блока логи позже logIndex `k` (не отменённые реоргом). Чистая. Ошибок нет.
pub fn later(logs: &[Log], k: u64) -> bool {
    logs.iter()
        .any(|l| !l.removed && l.log_index.is_some_and(|i| i > k))
}

/// Логи пула за один блок (eth_getLogs). Ошибки: узел, ответ не по виду.
async fn logs_of(
    ctx: &Ctx,
    pool: Address,
    block: u64,
) -> crate::gateway::errors::GatewayResult<Vec<Log>> {
    let tag = call::block_tag(block);
    let f = json!([{ "address": pool, "fromBlock": tag, "toBlock": tag }]);
    rpc::call(ctx, "eth_getLogs", f).await
}

/// Доказательство (см. шапку). Принимает: окружение, токены, реестр, снимок записи памяти. Возвращает: Proof. Ошибок
/// нет (сбой узла — Unproven с причиной).
pub async fn prove(ctx: &Ctx, tokens: &Tokens, reg: &Registry, mem: &Entry) -> Proof {
    let s = mem.synced_block;
    if let Some(k) = mem.synced_log {
        let PoolRef::Contract(addr) = &mem.pool.reference else {
            return Proof::Unproven(format!(
                "память посреди блока {s} (logIndex {k}), у пула нет своего адреса — логи блока не проверить"
            ));
        };
        match logs_of(ctx, *addr, s).await {
            Ok(logs) if later(&logs, k) => {
                return Proof::Unproven(format!(
                    "у пула есть логи блока {s} после logIndex {k} — память посреди действия (отставание)"
                ));
            }
            Ok(_) => {}
            Err(e) => {
                return Proof::Unproven(format!("eth_getLogs блока {s}: {}", e.message));
            }
        }
    }
    match load::refresh(ctx, tokens, reg, mem, s).await {
        Ok(node) if quoter_refetch_note::differs(mem, &node) => Proof::Wrong {
            note: quoter_refetch_note::note(mem, &node, s),
            ticks: quoter_refetch_note::ticks_differ(mem, &node),
        },
        Ok(_) => Proof::Right,
        Err(e) => Proof::Unproven(format!("чтение пула на блоке {s}: {}", e.message)),
    }
}
