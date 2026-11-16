//! Файл: venues/bsc_dex/pools/prefetch_integral.rs
//! Что делает: решение 325 — заранее читаемые вызовы (prefetch_plan.rs) пула Algebra Integral (THENA V3, DexSwap).
//! Чтение по одному (load_algebra_integral.rs) делает globalState, liquidity, [fee()], nextTickGlobal, prevTickGlobal
//! и затем ОБХОД СПИСКА тиков: ticks(t) по одному, следующий тик — из поля nextTick / prevTick ответа (walk) — до
//! dex.integral_tick_steps в каждую сторону; подряд это 4–5 + до 2 × dex.integral_tick_steps обращений, пачкой обход
//! не сделать: адрес следующего вызова есть только в ответе предыдущего. Но тики списка — ровно установленные биты
//! дерева поиска пула (TickTree.sol: лист переключается вместе со вставкой/удалением тика, TickStructure.sol L36-L64),
//! поэтому порядок обхода известен по битам заранее:
//! first — голова (те же 5 вызовов), весь второй слой дерева (какие листы не пусты; 28 узлов на весь диапазон тиков),
//! листы tickTable, где лежали тики записи и тик записи, и ticks() тиков записи;
//! more — по nextTickGlobal / prevTickGlobal из книги и битам листов: цепочка обхода (walk_plan — тот же порядок и та же
//! остановка, что у walk: не больше dex.integral_tick_steps тиков, конец списка — MAX_TICK / MIN_TICK) → ticks() её
//! тиков; не хватает прочитанного листа — его и следующие непустые листы (их не больше, чем осталось шагов).
//! Обычно 1–2 обращения, лист далеко — 3. Код обхода (walk) не меняется и сам проверяет каждое звено по ответу пула:
//! если подсказка разошлась со списком, недостающий тик он спросит у узла — итог тот же, что у чтения без книги.
//! Кто вызывает: pools/prefetch_plan.rs.
//! От чего зависит: abi_algebra_integral.rs, load_cl.rs (i24), math/tick_math.rs (MIN_TICK, MAX_TICK),
//! prefetch_plan.rs (enc, got), node/prefetch.rs (Book), registry.rs (Entry).
//! Настройки: dex.integral_tick_steps [bsc_dex] (как у load_algebra_integral.rs).
//!
//! ABI и индексы — abi_algebra_integral.rs (IAlgebraPoolState.sol L44, L57-L69, L99, L117, L143, L156, L161, L173;
//! TickTree.sol L10, _toggleBitInNode, getNextTick).

use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;

use super::super::ctx::Ctx;
use super::super::math::tick_math::{MAX_TICK, MIN_TICK};
use super::super::node::prefetch::{Book, Call};
use super::abi_algebra_integral::{IAlgebraIntegralPool as P, SECOND_LAYER_OFFSET};
use super::load_cl::i24;
use super::prefetch_plan::{enc, got};
use super::registry::Entry;

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

/// Голова, второй слой, листы и тики записи. Ошибок нет.
pub fn first(ctx: &Ctx, e: &Entry, pool: Address) -> Vec<Call> {
    let mut l = vec![
        enc(pool, &P::globalStateCall {}),
        enc(pool, &P::liquidityCall {}),
        enc(pool, &P::feeCall {}),
        enc(pool, &P::nextTickGlobalCall {}),
        enc(pool, &P::prevTickGlobalCall {}),
    ];
    l.extend(nodes().map(|node| enc(pool, &P::tickTreeSecondLayerCall { node })));
    let mut known: Vec<i32> = e
        .ticks
        .as_ref()
        .map(|m| m.ticks.keys().copied().collect())
        .unwrap_or_default();
    let mut leaves: Vec<i16> = known.iter().map(|t| leaf(*t)).collect();
    if let PoolState::Algebra { tick, .. } = e.pool.state {
        leaves.push(leaf(tick));
    }
    leaves.dedup();
    l.extend(
        leaves
            .into_iter()
            .map(|w| enc(pool, &P::tickTableCall { wordPosition: w })),
    );
    known.retain(|t| (MIN_TICK..=MAX_TICK).contains(t));
    l.extend(ticks_calls(ctx, pool, &known));
    l
}

/// Цепочка обхода по книге → ticks() её тиков и недостающие листы. Ошибок нет (нет ответа — пусто).
pub fn more(ctx: &Ctx, pool: Address, b: &Book) -> Vec<Call> {
    let Ok(steps) = ctx.int("dex.integral_tick_steps") else {
        return Vec::new();
    };
    let (Some(next), Some(prev)) = (
        got(b, pool, &P::nextTickGlobalCall {}),
        got(b, pool, &P::prevTickGlobalCall {}),
    ) else {
        return Vec::new();
    };
    let layer: Option<Vec<U256>> = nodes()
        .map(|node| got(b, pool, &P::tickTreeSecondLayerCall { node }))
        .collect();
    let Some(layer) = layer else {
        return Vec::new();
    };
    let ne = |l: i16| nonempty(&layer, l);
    let bits = |l: i16| got(b, pool, &P::tickTableCall { wordPosition: l });
    let mut out = Vec::new();
    for (start, up) in [(next.as_i32(), true), (prev.as_i32(), false)] {
        let p = walk_plan(start, up, steps, &ne, &bits);
        out.extend(ticks_calls(ctx, pool, &p.ticks));
        out.extend(
            p.need
                .into_iter()
                .map(|w| enc(pool, &P::tickTableCall { wordPosition: w })),
        );
    }
    out
}

/// ticks() тиков списком. Тик вне int24 пропускается (код чтения сам вернёт ошибку). Ошибок нет.
fn ticks_calls(ctx: &Ctx, pool: Address, ticks: &[i32]) -> Vec<Call> {
    ticks
        .iter()
        .filter_map(|t| i24(ctx, *t).ok())
        .map(|tick| enc(pool, &P::ticksCall { tick }))
        .collect()
}

/// Цепочка обхода одной стороны: тики, которые прочитает walk (load_algebra_integral.rs), и листы, которых не хватило.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub ticks: Vec<i32>,
    pub need: Vec<i16>,
}

/// Следующее звено: тик или «лист не прочитан».
enum Next {
    Tick(i32),
    Unknown(i16),
}

/// Порядок walk по битам дерева: от `start` не больше max(steps, 1) тиков; конец списка — MAX_TICK (вверх) /
/// MIN_TICK (вниз) включительно. Принимает: начало, сторону, шаги, «лист не пуст», биты листа (None — не прочитан).
/// Ошибок нет.
pub fn walk_plan(
    start: i32,
    up: bool,
    steps: u64,
    ne: &dyn Fn(i16) -> bool,
    bits: &dyn Fn(i16) -> Option<U256>,
) -> Plan {
    let mut p = Plan::default();
    let mut t = start;
    for _ in 0..steps.max(1) {
        p.ticks.push(t);
        if (up && t >= MAX_TICK) || (!up && t <= MIN_TICK) {
            break;
        }
        match neighbor(t, up, ne, bits) {
            Next::Tick(n) => t = n,
            Next::Unknown(l) => {
                let left = steps.max(1) as usize - p.ticks.len();
                p.need = leaves_from(l, up, left.max(1), ne);
                break;
            }
        }
    }
    p
}

/// Ближайший установленный бит строго выше (up) / ниже `t`; битов нет до края — MAX_TICK / MIN_TICK. Ошибок нет.
fn neighbor(
    t: i32,
    up: bool,
    ne: &dyn Fn(i16) -> bool,
    bits: &dyn Fn(i16) -> Option<U256>,
) -> Next {
    let (w0, b0) = (leaf(t), (t & 0xFF) as usize);
    let (lo, hi) = (leaf(MIN_TICK), leaf(MAX_TICK));
    let mut w = w0;
    loop {
        if ne(w) {
            let Some(word) = bits(w) else {
                return Next::Unknown(w);
            };
            let found = if up {
                let from = if w == w0 { b0 + 1 } else { 0 };
                (from..256).find(|i| word.bit(*i))
            } else {
                let to = if w == w0 { b0 } else { 256 };
                (0..to).rev().find(|i| word.bit(*i))
            };
            if let Some(i) = found {
                return Next::Tick(i32::from(w) * 256 + i as i32);
            }
        }
        if (up && w >= hi) || (!up && w <= lo) {
            return Next::Tick(if up { MAX_TICK } else { MIN_TICK });
        }
        w = if up { w + 1 } else { w - 1 };
    }
}

/// Непустые листы от `l` в сторону обхода, не больше `n`. Ошибок нет.
fn leaves_from(l: i16, up: bool, n: usize, ne: &dyn Fn(i16) -> bool) -> Vec<i16> {
    let (lo, hi) = (leaf(MIN_TICK), leaf(MAX_TICK));
    let mut out = Vec::new();
    let mut w = l;
    while out.len() < n {
        if ne(w) {
            out.push(w);
        }
        if (up && w >= hi) || (!up && w <= lo) {
            break;
        }
        w = if up { w + 1 } else { w - 1 };
    }
    out
}
