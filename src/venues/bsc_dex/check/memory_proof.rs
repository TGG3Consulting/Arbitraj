//! Файл: venues/bsc_dex/check/memory_proof.rs
//! Что делает: решение 382 (ревью Б1, ревью 2 Б1, ревью 3 Б1/Б2/Н1/Н2) — доказательство, что память пула НЕВЕРНА, а не
//! просто отстаёт (правила — check/memory_proof_rules.rs):
//! 1. вид «каждым блоком» — сравнение с узлом только на блоке памяти (S = H), иначе отставание;
//! 2. логи пула за [S, H] одним eth_getLogs (фильтр подписки пачки); неприменённый памятью лог старше H −
//!    dex.quoter_memory_lag_blocks — пропуск события у пула, чьи логи это доказывают (own), ошибка доказана (Wrong с
//!    позицией пропуска — вызывающий метит, только если память так и не применила его, ревью 3 Н1); только свежие —
//!    отставание;
//! 3. узел не отдал логи за [S, H] (предел диапазона своего узла — 5000 блоков, tests/data/node_limits_2026-11-12; или
//!    сбой) — узкое окно [max(S, H − отставание), H]: свежие неприменённые — отставание; нет — дальше п. 4 (у
//!    событийного вида без свежих событий память обязана совпасть с узлом на H; старый пропуск даёт расхождение);
//! 4. неприменённых нет — сравнение памяти с пулом, прочитанным на H (верх, карта тиков в общих словах). Различаются —
//!    Wrong (сильная, если неверна карта тиков); равны — Right.
//!
//! Не задана dex.quoter_memory_lag_blocks — доказательство не выполняется (Unproven с её именем — видно в журнале).
//! Кто вызывает: exec/quoter_refetch_proof.rs (judge — фоном и перед сделкой, и по «память отстаёт» сверки).
//! От чего зависит: check/memory_proof_rules.rs, node/rpc.rs, pools/load.rs, pools/registry.rs,
//! exec/quoter_refetch_note.rs, check/quoter_block.rs (MEMORY_LAG), pools/fusion_book.rs (isolated — решение 463).
//! Настройки: dex.quoter_memory_lag_blocks [bsc_dex]; через pools/load.rs — настройки чтения вида пула.
//!
//! eth_getLogs(filter: address, topics, fromBlock, toBlock) — execution-apis
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth (eth_getLogs).

use alloy::rpc::types::Log;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::exec::quoter_refetch_note;
use super::super::node::rpc;
use super::super::pools::{fusion_book, load};
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;
use super::memory_proof_rules::{self as rules, Unapplied};
use super::quoter_block::MEMORY_LAG;

/// Итог доказательства.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proof {
    /// Память неверна: что не так; «сильная» ли ошибка (пропуск события или карта тиков — следующий Swap её не
    /// поправит); позиция пропущенного лога (Some — ошибка доказана пропуском).
    Wrong {
        note: String,
        strong: bool,
        missed: Option<(u64, u64)>,
    },
    /// Память отстаёт (события в пути) — причина; не ошибка.
    Lag(String),
    /// Память равна узлу на H.
    Right,
    /// Доказать нельзя — причина.
    Unproven(String),
}

/// Логи пула за блоки [from, to] (eth_getLogs). Ошибки: узел (в том числе предел диапазона), ответ не по виду.
async fn logs_of(ctx: &Ctx, e: &Entry, from: u64, to: u64) -> GatewayResult<Vec<Log>> {
    let f = rules::filter(e, from, to);
    let mut logs: Vec<Log> = rpc::call(ctx, "eth_getLogs", serde_json::json!([f])).await?;
    // Ревью 447: у Ekubo фильтр — все логи ядра, логи пула — по PoolId из данных.
    logs.retain(|l| rules::mine(e, l));
    Ok(logs)
}

/// Итог по разбору логов (чистая): пропуск у own-пула — Wrong с позицией; неприменённые — Lag; нет — None (дальше
/// сравнение на H). Принимает: запись памяти, разбор, голову, допуск. Ошибок нет.
pub fn by_logs(mem: &Entry, u: Unapplied, head: u64, lag: u64) -> Option<Proof> {
    match u {
        Unapplied::None => None,
        Unapplied::Missed((b, i)) if rules::own(mem) => Some(Proof::Wrong {
            note: format!(
                "{} {}: событие пула блок {b}, logIndex {i} есть у узла, память его не применила (память — блок {}{}, \
                 голова {head}, допуск отставания {MEMORY_LAG} = {lag})",
                mem.family.title,
                mem.ticker,
                mem.synced_block,
                mem.synced_log
                    .map_or(String::new(), |k| format!(", logIndex {k}"))
            ),
            strong: true,
            missed: Some((b, i)),
        }),
        Unapplied::Missed((b, i)) | Unapplied::Fresh(_, (b, i)) => Some(Proof::Lag(format!(
            "есть события пула, не применённые памятью, самое раннее — блок {b}, logIndex {i}"
        ))),
    }
}

/// Доказательство (см. шапку). Принимает: окружение, (токены, реестр), запись памяти, голову H, пул, прочитанный на H
/// (None — прочитать здесь). Возвращает: Proof. Ошибок нет (сбой узла — Unproven с причиной).
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
    if let Some(why) = rules::per_block_gate(mem, head) {
        return Proof::Lag(why);
    }
    let applied = |b: u64, i: u64| mem.has_applied(b, i);
    let u = match logs_of(ctx, mem, s, head).await {
        Ok(l) => rules::unapplied(&rules::positions(&l), applied, head, lag),
        // Ревью 3 Б2: весь диапазон не отдан — узкое окно свежих логов, дальше сравнение на H.
        Err(wide) => match logs_of(ctx, mem, rules::narrow_from(s, head, lag), head).await {
            // В узком окне всё свежее: неприменённое — отставание, не пропуск.
            Ok(l) => match rules::unapplied(&rules::positions(&l), applied, head, lag) {
                Unapplied::Missed(p) => Unapplied::Fresh(1, p),
                x => x,
            },
            Err(e) => {
                return Proof::Unproven(format!(
                    "eth_getLogs блоков {s}..{head}: {}; узкое окно: {}",
                    wide.message, e.message
                ));
            }
        },
    };
    if let Some(p) = by_logs(mem, u, head, lag) {
        return p;
    }
    let read;
    let node = match at_head {
        Some(n) => n,
        // Решение 463 (повторное ревью t236): оракул THENA Fusion этого чтения — не в книгу (fusion_book.rs).
        None => match fusion_book::isolated(load::refresh(ctx, tokens, reg, mem, head)).await {
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
            missed: None,
        }
    } else {
        Proof::Right
    }
}
