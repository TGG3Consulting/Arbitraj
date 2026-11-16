//! Файл: venues/bsc_dex/pools/prefetch_walk.rs
//! Что делает: решение 325 — порядок обхода списка тиков Algebra Integral (walk в load_algebra_integral.rs) по битам
//! дерева тиков, без обращений к узлу: walk_plan — тики, которые прочитает walk (не больше dex.integral_tick_steps, конец
//! списка — MAX_TICK / MIN_TICK), и листы дерева, которых не хватило (их не больше, чем осталось шагов). Вынесено из
//! prefetch_integral.rs (ревью 325, Н8: предел размера файла).
//! Кто вызывает: pools/prefetch_integral.rs (more), tests/bsc_dex_prefetch_325.rs.
//! От чего зависит: prefetch_integral.rs (leaf), math/tick_math.rs (MIN_TICK, MAX_TICK).
//! Настроек нет (шаги передаёт вызывающий).
//!
//! Индексы дерева — TickTree.sol integral-core 1.2.10:
//! https://github.com/cryptoalgebra/Algebra/blob/1ff43e11407872f7c09b001cdeee1a59d2c334ec/src/core/contracts/libraries/TickTree.sol
//! (лист = sar(8, tick), бит = tick & 0xFF; abi_algebra_integral.rs).

use alloy_primitives::U256;

use super::super::math::tick_math::{MAX_TICK, MIN_TICK};
use super::prefetch_integral::leaf;

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
