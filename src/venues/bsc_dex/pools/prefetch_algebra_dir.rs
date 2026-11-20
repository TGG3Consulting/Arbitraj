//! Файл: venues/bsc_dex/pools/prefetch_algebra_dir.rs
//! Что делает: решение 444 — заранее читаемые вызовы (решение 325, prefetch_plan.rs) пула Algebra V1.9 с
//! направленными комиссиями (Camelot V3): то же, что у THENA Fusion (prefetch_ticks.rs algebra_first / algebra_more),
//! но тик берётся из globalState своего ABI (8 значений — abi_algebra_dir.rs). Селектор globalState() у обоих ABI один
//! (те же байты вызова, что у чтения load_algebra_dir.rs), отличается только разбор ответа.
//! Кто вызывает: pools/prefetch_plan.rs.
//! От чего зависит: abi_algebra_dir.rs, abi_algebra.rs (liquidity), load_algebra.rs (raw_word_range),
//! prefetch_ticks.rs (algebra_words, algebra_more_at), prefetch_plan.rs (enc, got), node/prefetch.rs (Book).
//! Настройки: через вызываемые — dex.tick_words, dex.tick_read_band_pct, dex.tick_read_band_algebra_pct.

use alloy_primitives::Address;

use crate::core::types::PoolState;

use super::super::ctx::Ctx;
use super::super::node::prefetch::{Book, Call};
use super::abi_algebra::IAlgebraPool;
use super::abi_algebra_dir::IAlgebraDirPool;
use super::load_algebra::raw_word_range;
use super::prefetch_plan::{enc, got};
use super::prefetch_ticks::{algebra_more_at, algebra_words};
use super::registry::Entry;

/// globalState, liquidity и сырые слова tickTable вокруг тика записи. Принимает: окружение, запись, пул.
/// Возвращает: вызовы. Ошибок нет (нет тика — только голова).
pub fn first(ctx: &Ctx, e: &Entry, pool: Address) -> Vec<Call> {
    let mut l = vec![
        enc(pool, &IAlgebraDirPool::globalStateCall {}),
        enc(pool, &IAlgebraPool::liquidityCall {}),
    ];
    if let PoolState::Algebra { tick, .. } = &e.pool.state
        && let Ok(r) = raw_word_range(ctx, *tick)
    {
        l.extend(algebra_words(pool, r));
    }
    l
}

/// По globalState из книги — слова настоящего диапазона; все в книге — ticks() по их битам. Принимает: окружение, пул,
/// книгу. Возвращает: вызовы (пусто — globalState ещё нет). Ошибок нет.
pub fn more(ctx: &Ctx, pool: Address, b: &Book) -> Vec<Call> {
    match got(b, pool, &IAlgebraDirPool::globalStateCall {}) {
        Some(gs) => algebra_more_at(ctx, pool, b, gs.tick.as_i32()),
        None => Vec::new(),
    }
}
