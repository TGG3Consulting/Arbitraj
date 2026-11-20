//! Файл: venues/bsc_dex/pools/prefetch_ticks.rs
//! Что делает: решение 325 — заранее читаемые вызовы (prefetch_plan.rs) пулов, чьё чтение по одному идёт «голова →
//! слова битмапа вокруг тика → данные тиков по битам»: Slipstream (load_slipstream.rs: slot0, liquidity, tickBitmap,
//! ticks) и Algebra V1.9 / THENA Fusion (load_algebra.rs: globalState, liquidity, сырой битмап tickTable, ticks).
//! first — голова и слова вокруг тика ЗАПИСИ (off_map — после события, Refresh — до; подсказка); more — по голове из
//! книги слова НАСТОЯЩЕГО диапазона
//! (та же функция диапазона, что у чтения: load_v2v3.rs word_range / load_algebra.rs raw_word_range), а когда все
//! они в книге — данные тиков по их битам (load_cl.rs set_ticks / load_algebra.rs raw_ticks). Вызовы — те же sol!, что
//! у чтения по одному (те же байты), поэтому код чтения берёт ответы из книги: обычно два обращения (голова + слова;
//! тики), тик ушёл из слов записи — три. Без книги было 4 подряд.
//! Кто вызывает: pools/prefetch_plan.rs.
//! От чего зависит: abi_slipstream.rs (ICLPool), abi_algebra.rs (IAlgebraPool), load_cl.rs (set_ticks, i24),
//! load_v2v3.rs (word_range), load_algebra.rs (raw_word_range, raw_ticks), prefetch_plan.rs (enc, got),
//! node/prefetch.rs (Book).
//! Настройки: через вызываемые — dex.tick_words, dex.tick_read_band_pct, dex.tick_read_band_algebra_pct.
//!
//! ABI: Slipstream — ICLPool (CLPool.sol slot0/liquidity/tickBitmap/ticks; abi_slipstream.rs); Algebra V1.9 —
//! @cryptoalgebra/v1.9-core IAlgebraPoolState.sol globalState/liquidity/tickTable/ticks (abi_algebra.rs).

use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;

use super::super::ctx::Ctx;
use super::super::node::prefetch::{Book, Call};
use super::abi_algebra::IAlgebraPool;
use super::abi_slipstream::ICLPool;
use super::load_algebra::{raw_ticks, raw_word_range};
use super::load_cl::{i24, set_ticks};
use super::load_v2v3::word_range;
use super::prefetch_plan::{enc, got};
use super::registry::Entry;

/// Slipstream: slot0, liquidity и слова tickBitmap вокруг тика записи. Ошибок нет (нет тика/шага — только голова).
pub fn slip_first(ctx: &Ctx, e: &Entry, pool: Address) -> Vec<Call> {
    let mut l = vec![
        enc(pool, &ICLPool::slot0Call {}),
        enc(pool, &ICLPool::liquidityCall {}),
    ];
    if let (PoolState::V3 { tick, .. }, Some(sp)) = (&e.pool.state, e.pool.tick_spacing)
        && let Ok(r) = word_range(ctx, *tick, sp)
    {
        l.extend(slip_words(pool, r));
    }
    l
}

/// Slipstream: по slot0 из книги — слова настоящего диапазона; все в книге — ticks() установленных тиков.
/// Ошибок нет (нет ответа — пусто).
pub fn slip_more(ctx: &Ctx, e: &Entry, pool: Address, b: &Book) -> Vec<Call> {
    let (Some(s), Some(sp)) = (got(b, pool, &ICLPool::slot0Call {}), e.pool.tick_spacing) else {
        return Vec::new();
    };
    let Ok((lo, hi)) = word_range(ctx, s.tick.as_i32(), sp) else {
        return Vec::new();
    };
    let mut calls = slip_words(pool, (lo, hi));
    let words: Option<Vec<U256>> = (lo..=hi)
        .map(|w| got(b, pool, &ICLPool::tickBitmapCall { wordPosition: w }))
        .collect();
    if let Some(words) = words {
        calls.extend(
            set_ticks(lo, &words, sp)
                .iter()
                .filter_map(|t| i24(ctx, *t).ok())
                .map(|tick| enc(pool, &ICLPool::ticksCall { tick })),
        );
    }
    calls
}

/// Вызовы слов tickBitmap [lo, hi] пула Slipstream. Ошибок нет.
fn slip_words(pool: Address, (lo, hi): (i16, i16)) -> Vec<Call> {
    (lo..=hi)
        .map(|w| enc(pool, &ICLPool::tickBitmapCall { wordPosition: w }))
        .collect()
}

/// Algebra V1.9: globalState, liquidity и сырые слова tickTable вокруг тика записи. Ошибок нет.
pub fn algebra_first(ctx: &Ctx, e: &Entry, pool: Address) -> Vec<Call> {
    let mut l = vec![
        enc(pool, &IAlgebraPool::globalStateCall {}),
        enc(pool, &IAlgebraPool::liquidityCall {}),
    ];
    if let PoolState::Algebra { tick, .. } = &e.pool.state
        && let Ok(r) = raw_word_range(ctx, *tick)
    {
        l.extend(algebra_words(pool, r));
    }
    l
}

/// Algebra V1.9: по globalState из книги — слова настоящего диапазона; все в книге — ticks() по их битам.
/// Ошибок нет (нет ответа — пусто).
pub fn algebra_more(ctx: &Ctx, pool: Address, b: &Book) -> Vec<Call> {
    let Some(gs) = got(b, pool, &IAlgebraPool::globalStateCall {}) else {
        return Vec::new();
    };
    algebra_more_at(ctx, pool, b, gs.tick.as_i32())
}

/// Algebra V1.9 (и с направленными комиссиями — решение 444, prefetch_algebra_dir.rs): по тику globalState — слова
/// настоящего диапазона; все в книге — ticks() по их битам. Принимает: окружение, пул, книгу, тик. Ошибок нет.
pub fn algebra_more_at(ctx: &Ctx, pool: Address, b: &Book, tick: i32) -> Vec<Call> {
    let Ok((lo, hi)) = raw_word_range(ctx, tick) else {
        return Vec::new();
    };
    let mut calls = algebra_words(pool, (lo, hi));
    let words: Option<Vec<U256>> = (lo..=hi)
        .map(|w| got(b, pool, &IAlgebraPool::tickTableCall { wordPosition: w }))
        .collect();
    if let Some(words) = words {
        calls.extend(
            raw_ticks(lo, &words)
                .iter()
                .filter_map(|t| i24(ctx, *t).ok())
                .map(|tick| enc(pool, &IAlgebraPool::ticksCall { tick })),
        );
    }
    calls
}

/// Вызовы сырых слов tickTable [lo, hi] пула Algebra V1.9 (и с направленными комиссиями). Ошибок нет.
fn algebra_words(pool: Address, (lo, hi): (i16, i16)) -> Vec<Call> {
    (lo..=hi)
        .map(|w| enc(pool, &IAlgebraPool::tickTableCall { wordPosition: w }))
        .collect()
}
