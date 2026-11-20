//! Файл: venues/bsc_dex/pools/prefetch_algebra_dir.rs
//! Что делает: решение 444 — заранее читаемые вызовы (решение 325, prefetch_plan.rs) пула Algebra V1.9 с
//! направленными комиссиями (Camelot V3): то же, что у THENA Fusion (prefetch_ticks.rs algebra_first / algebra_more),
//! но тик берётся из globalState своего ABI (8 значений — abi_algebra_dir.rs). Селектор globalState() у обоих ABI один
//! (те же байты вызова, что у чтения load_algebra_dir.rs), отличается только разбор ответа.
//! Кто вызывает: pools/prefetch_plan.rs.
//! От чего зависит: abi_algebra_dir.rs, prefetch_ticks.rs (algebra_first, algebra_more_at), prefetch_plan.rs (got),
//! node/prefetch.rs (Book).
//! Настройки: через вызываемые — dex.tick_words, dex.tick_read_band_pct, dex.tick_read_band_algebra_pct.

use alloy_primitives::Address;

use super::super::ctx::Ctx;
use super::super::node::prefetch::{Book, Call};
use super::abi_algebra_dir::IAlgebraDirPool;
use super::prefetch_plan::got;
use super::prefetch_ticks::{algebra_first, algebra_more_at};
use super::registry::Entry;

/// globalState, liquidity и сырые слова tickTable вокруг тика записи — ровно prefetch_ticks::algebra_first (ревью 444:
/// селектор globalState() у обоих ABI один, байты вызовов те же). Принимает: окружение, запись, пул. Возвращает:
/// вызовы. Ошибок нет (нет тика — только голова).
pub fn first(ctx: &Ctx, e: &Entry, pool: Address) -> Vec<Call> {
    algebra_first(ctx, e, pool)
}

/// По globalState из книги — слова настоящего диапазона; все в книге — ticks() по их битам. Принимает: окружение, пул,
/// книгу. Возвращает: вызовы (пусто — globalState ещё нет). Ошибок нет.
pub fn more(ctx: &Ctx, pool: Address, b: &Book) -> Vec<Call> {
    match got(b, pool, &IAlgebraDirPool::globalStateCall {}) {
        Some(gs) => algebra_more_at(ctx, pool, b, gs.tick.as_i32()),
        None => Vec::new(),
    }
}
