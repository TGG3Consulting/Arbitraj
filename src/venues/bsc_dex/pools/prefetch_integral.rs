//! Файл: venues/bsc_dex/pools/prefetch_integral.rs
//! Что делает: решение 325 — заранее читаемые вызовы (prefetch_plan.rs) пула Algebra Integral (THENA V3, DexSwap).
//! Чтение по одному (load_algebra_integral.rs) делает globalState, liquidity, [fee()], nextTickGlobal, prevTickGlobal
//! и затем ОБХОД СПИСКА тиков: ticks(t) по одному, следующий тик — из поля nextTick / prevTick ответа (walk) — до
//! dex.integral_tick_steps в каждую сторону; подряд это 4–5 + до 2 × dex.integral_tick_steps обращений, пачкой обход
//! не сделать: адрес следующего вызова есть только в ответе предыдущего. Но тики списка — ровно установленные биты
//! дерева поиска пула (TickTree.sol: лист переключается вместе со вставкой/удалением тика, TickStructure.sol L36-L64),
//! поэтому порядок обхода известен по битам заранее:
//! first — нужное коду чтения: голова (globalState, liquidity, nextTickGlobal, prevTickGlobal); вспомогательное (в
//! порядке): весь второй слой дерева (какие листы не пусты; 28 узлов на весь диапазон тиков), листы tickTable, где
//! лежали тики записи и тик записи. Ревью 325 (Н5): ticks() тиков карты заранее НЕ читаются (их даёт цепочка), fee() —
//! только при DYNAMIC_FEE (more, по globalState из книги): так нужное — ровно поодиночные вызовы кода чтения;
//! more — нужное: fee() при DYNAMIC_FEE и ticks() цепочки обхода по nextTickGlobal / prevTickGlobal из книги и битам
//! листов (prefetch_walk.rs walk_plan — тот же порядок и та же остановка, что у walk: не больше
//! dex.integral_tick_steps тиков, конец списка — MAX_TICK / MIN_TICK); вспомогательное: листы, которых не хватило (их не
//! больше, чем осталось шагов). Вспомогательное берётся, только пока частей обращения не больше нужных вызовов
//! (prefetch_ask.rs pick) — обращений не больше, чем у чтения по одному, при любом dex.load_batch.
//! Обычно 1–2 обращения, лист далеко — 3. Код обхода (walk) не меняется и сам проверяет каждое звено по ответу пула:
//! если подсказка разошлась со списком, недостающий тик он спросит у узла — итог тот же, что у чтения без книги.
//! Кто вызывает: pools/prefetch_plan.rs; tests/bsc_dex_prefetch_325.rs (leaf, nodes, nonempty).
//! От чего зависит: abi_algebra_integral.rs, load_cl.rs (i24), math/tick_math.rs (MIN_TICK, MAX_TICK),
//! prefetch_plan.rs (enc, got), prefetch_ask.rs (Ask), prefetch_walk.rs (walk_plan), integral_fee.rs (dynamic),
//! node/prefetch.rs (Book), registry.rs (Entry).
//! Настройки: dex.integral_tick_steps [bsc_dex] (как у load_algebra_integral.rs).
//!
//! ABI и индексы — abi_algebra_integral.rs: IAlgebraPoolState.sol
//! https://github.com/cryptoalgebra/Algebra/blob/1ff43e11407872f7c09b001cdeee1a59d2c334ec/src/core/contracts/interfaces/pool/IAlgebraPoolState.sol
//! (L44, L57-L69, L99, L117, L143, L156, L161, L173); TickTree.sol
//! https://github.com/cryptoalgebra/Algebra/blob/1ff43e11407872f7c09b001cdeee1a59d2c334ec/src/core/contracts/libraries/TickTree.sol
//! (L10, _toggleBitInNode, getNextTick).

use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;

use super::super::ctx::Ctx;
use super::super::math::tick_math::{MAX_TICK, MIN_TICK};
use super::super::node::prefetch::{Book, Call};
use super::abi_algebra_integral::{IAlgebraIntegralPool as P, SECOND_LAYER_OFFSET};
use super::load_cl::i24;
use super::prefetch_ask::Ask;
use super::prefetch_plan::{enc, got};
use super::registry::Entry;

pub use super::prefetch_walk::{Plan, walk_plan};

/// Лист дерева (tickTable) сырого тика: sar(8, tick). Ошибок нет.
pub fn leaf(t: i32) -> i16 {
    (t >> 8) as i16
}

/// Узлы второго слоя на весь диапазон тиков [MIN_TICK; MAX_TICK]. Ошибок нет.
pub fn nodes() -> std::ops::RangeInclusive<i16> {
    let n = |t: i32| ((i32::from(leaf(t)) + SECOND_LAYER_OFFSET) >> 8) as i16;
    n(MIN_TICK)..=n(MAX_TICK)
}

/// Лист не пуст по второму слою. Принимает: узлы второго слоя (с первого из nodes()), лист. Ошибок нет.
pub fn nonempty(layer: &[U256], l: i16) -> bool {
    let i = i32::from(l) + SECOND_LAYER_OFFSET;
    let first = i32::from(*nodes().start());
    layer
        .get(((i >> 8) - first) as usize)
        .is_some_and(|w| w.bit((i & 0xFF) as usize))
}

/// Голова (нужное), второй слой и листы записи (вспомогательное). Ошибок нет.
pub fn first(e: &Entry, pool: Address) -> Ask {
    let useful = vec![
        enc(pool, &P::globalStateCall {}),
        enc(pool, &P::liquidityCall {}),
        enc(pool, &P::nextTickGlobalCall {}),
        enc(pool, &P::prevTickGlobalCall {}),
    ];
    let layer: Vec<Call> = nodes()
        .map(|node| enc(pool, &P::tickTreeSecondLayerCall { node }))
        .collect();
    let mut leaves: Vec<i16> = e
        .ticks
        .as_ref()
        .map(|m| m.ticks.keys().map(|t| leaf(*t)).collect())
        .unwrap_or_default();
    if let PoolState::Algebra { tick, .. } = e.pool.state {
        leaves.push(leaf(tick));
    }
    leaves.sort_unstable();
    leaves.dedup();
    let words: Vec<Call> = leaves
        .into_iter()
        .map(|w| enc(pool, &P::tickTableCall { wordPosition: w }))
        .collect();
    Ask {
        useful,
        extra: vec![layer, words],
    }
}

/// fee() при DYNAMIC_FEE и цепочка обхода по книге (нужное), недостающие листы (вспомогательное). Ошибок нет (нет
/// ответа — пусто).
pub fn more(ctx: &Ctx, pool: Address, b: &Book) -> Ask {
    let mut ask = Ask::default();
    if let Some(gs) = got(b, pool, &P::globalStateCall {})
        && super::integral_fee::dynamic(gs.pluginConfig)
    {
        ask.useful.push(enc(pool, &P::feeCall {}));
    }
    let Ok(steps) = ctx.int("dex.integral_tick_steps") else {
        return ask;
    };
    let (Some(next), Some(prev)) = (
        got(b, pool, &P::nextTickGlobalCall {}),
        got(b, pool, &P::prevTickGlobalCall {}),
    ) else {
        return ask;
    };
    let layer: Option<Vec<U256>> = nodes()
        .map(|node| got(b, pool, &P::tickTreeSecondLayerCall { node }))
        .collect();
    let Some(layer) = layer else {
        return ask;
    };
    let ne = |l: i16| nonempty(&layer, l);
    let bits = |l: i16| got(b, pool, &P::tickTableCall { wordPosition: l });
    let mut need = Vec::new();
    for (start, up) in [(next.as_i32(), true), (prev.as_i32(), false)] {
        let p = walk_plan(start, up, steps, &ne, &bits);
        ask.useful.extend(ticks_calls(ctx, pool, &p.ticks));
        need.extend(
            p.need
                .into_iter()
                .map(|w| enc(pool, &P::tickTableCall { wordPosition: w })),
        );
    }
    ask.extra.push(need);
    ask
}

/// ticks() тиков списком. Тик вне int24 пропускается (код чтения сам вернёт ошибку). Ошибок нет.
fn ticks_calls(ctx: &Ctx, pool: Address, ticks: &[i32]) -> Vec<Call> {
    ticks
        .iter()
        .filter_map(|t| i24(ctx, *t).ok())
        .map(|tick| enc(pool, &P::ticksCall { tick }))
        .collect()
}
