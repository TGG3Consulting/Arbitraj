//! Файл: venues/bsc_dex/pools/prefetch_ticks.rs
//! Что делает: решение 325 — заранее читаемые вызовы (prefetch_plan.rs) пулов, чьё чтение по одному идёт «голова →
//! слова битмапа вокруг тика → данные тиков по битам»: Slipstream (load_slipstream.rs: slot0, liquidity, tickBitmap,
//! ticks) и THENA Fusion (load_algebra.rs: globalState, liquidity, tickSpacing, сжатый битмап tickTable, ticks —
//! решение 463; Algebra V1.9 Camelot V3 — тот же путь при шаге 1, сырой битмап, prefetch_algebra_dir.rs).
//! first — голова и слова вокруг тика ЗАПИСИ (off_map — после события, Refresh — до; подсказка); more — по голове из
//! книги слова НАСТОЯЩЕГО диапазона
//! (та же функция диапазона, что у чтения: load_v2v3.rs word_range / load_algebra.rs row_range), а когда все
//! они в книге — данные тиков по их битам (load_cl.rs set_ticks / load_algebra.rs row_ticks). Вызовы — те же sol!, что
//! у чтения по одному (те же байты), поэтому код чтения берёт ответы из книги: обычно два обращения (голова + слова;
//! тики), тик ушёл из слов записи — три. Без книги было 4 подряд.
//! Кто вызывает: pools/prefetch_plan.rs.
//! От чего зависит: abi_slipstream.rs (ICLPool), abi_algebra.rs (IAlgebraPool), load_cl.rs (set_ticks, i24),
//! load_v2v3.rs (word_range), load_algebra.rs (row_range, row_ticks, spacing_of), prefetch_plan.rs (enc, got),
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
use super::load_algebra::{row_range, row_ticks, spacing_of};
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

/// tickSpacing пула THENA Fusion, известный записи (ревью t236: Pool.tick_spacing — load.rs known_spacing, иначе шаг
/// прочитанной карты). None — ещё не читался. Ошибок нет.
fn algebra_spacing(e: &Entry) -> Option<i32> {
    if e.family.variant==super::family_variant::Variant::AlgebraSingleV19 {return Some(1);}
    let map = e.ticks.as_ref().map(|m| m.spacing);
    e.pool.tick_spacing.or(map)
}

/// THENA Fusion (решение 463): globalState, liquidity и сжатые слова tickTable вокруг тика записи при известном шаге
/// (ревью t236: слова — в первом заходе; tickSpacing() — только если шаг ещё не известен, как в load_algebra.rs).
/// Ошибок нет.
pub fn algebra_first(ctx: &Ctx, e: &Entry, pool: Address) -> Vec<Call> {
    match algebra_spacing(e) {
        Some(sp) => algebra_first_at(ctx, e, pool, sp),
        None => vec![
            enc(pool, &IAlgebraPool::globalStateCall {}),
            enc(pool, &IAlgebraPool::liquidityCall {}),
            enc(pool, &IAlgebraPool::tickSpacingCall {}),
        ],
    }
}

/// globalState, liquidity и слова tickTable вокруг тика записи при шаге карты `sp` (THENA Fusion — tickSpacing пула;
/// Algebra V1.9 с направленными комиссиями, prefetch_algebra_dir.rs, — 1: сырой битмап). Ошибок нет.
pub fn algebra_first_at(ctx: &Ctx, e: &Entry, pool: Address, sp: i32) -> Vec<Call> {
    let mut l = vec![
        enc(pool, &IAlgebraPool::globalStateCall {}),
        enc(pool, &IAlgebraPool::liquidityCall {}),
    ];
    if let PoolState::Algebra { tick, .. } = &e.pool.state
        && let Ok(r) = row_range(ctx, *tick, sp)
    {
        l.extend(algebra_words(pool, r));
    }
    l
}

/// THENA Fusion (решение 463): по globalState из книги и шагу (известному записи или tickSpacing из книги) — сжатые
/// слова настоящего диапазона (как load_algebra.rs); все в книге — ticks() по их битам. Ошибок нет (нет ответа —
/// пусто).
pub fn algebra_more(ctx: &Ctx, e: &Entry, pool: Address, b: &Book) -> Vec<Call> {
    let Some(gs) = got(b, pool, &IAlgebraPool::globalStateCall {}) else {
        return Vec::new();
    };
    let read = got(b, pool, &IAlgebraPool::tickSpacingCall {});
    let read = read.map(|s| s.as_i32());
    let Some(sp) = algebra_spacing(e).or(read) else {
        return Vec::new();
    };
    let Ok(sp) = spacing_of(ctx, sp) else {
        return Vec::new();
    };
    algebra_more_at(ctx, pool, b, gs.tick.as_i32(), sp)
}

/// По тику globalState и шагу карты `sp` (THENA Fusion — tickSpacing пула; Algebra V1.9 с направленными комиссиями —
/// 1, решение 444, prefetch_algebra_dir.rs) — слова настоящего диапазона (load_algebra.rs row_range); все в книге —
/// ticks() по их битам. Принимает: окружение, пул, книгу, тик, шаг. Ошибок нет.
pub fn algebra_more_at(ctx: &Ctx, pool: Address, b: &Book, tick: i32, sp: i32) -> Vec<Call> {
    let Ok((lo, hi)) = row_range(ctx, tick, sp) else {
        return Vec::new();
    };
    let mut calls = algebra_words(pool, (lo, hi));
    let words: Option<Vec<U256>> = (lo..=hi)
        .map(|w| got(b, pool, &IAlgebraPool::tickTableCall { wordPosition: w }))
        .collect();
    if let Some(words) = words {
        calls.extend(
            row_ticks(lo, &words, sp)
                .iter()
                .filter_map(|t| i24(ctx, *t).ok())
                .map(|tick| enc(pool, &IAlgebraPool::ticksCall { tick })),
        );
    }
    calls
}

/// Вызовы слов tickTable [lo, hi] пула Algebra (THENA Fusion — сжатые, Camelot V3 — сырые). Ошибок нет.
fn algebra_words(pool: Address, (lo, hi): (i16, i16)) -> Vec<Call> {
    (lo..=hi)
        .map(|w| enc(pool, &IAlgebraPool::tickTableCall { wordPosition: w }))
        .collect()
}
