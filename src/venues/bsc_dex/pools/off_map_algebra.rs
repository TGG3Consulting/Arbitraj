//! Файл: venues/bsc_dex/pools/off_map_algebra.rs
//! Что делает: решение 320 — выход полосы глубины за прочитанную карту тиков у пулов Algebra (PoolState::Algebra: THENA
//! Fusion — Algebra V1/V1.9, и Algebra Integral). У V3/CL/V4 его ловит depth.rs off_map; у Algebra до решения 320 карта
//! перечитывалась только на Mint/Burn (events_algebra.rs), и после большого Swap, уведшего тик за окно, расчёт обмена и
//! глубины (math/swap_algebra.rs swap_to) шёл неверно: в сторону от окна — шаг в обратную сторону до края окна, к окну —
//! постоянная ликвидность на непрочитанном отрезке. Здесь — только «включено ли» и «поможет ли перечитывание»; сама
//! проверка полосы и перечитывание — те же, что у V3/CL (depth.rs off_map → pools/listen_log.rs ветка off_map →
//! load::refresh, тот же путь, что перечитывание Algebra на Mint/Burn).
//! Поможет ли перечитывание (без него каждый Swap перечитывал бы пул, у которого полоса шире окна по настройкам):
//! - THENA Fusion (решение 462): окно сжатого битмапа вокруг нового тика (load_algebra::row_range при шаге карты)
//!   не совпадает с прочитанным — перечитывание сдвинет окно; совпадает — ничего не добавит (не чаще раза на слово =
//!   256·spacing тиков);
//! - Algebra Integral: отрезок списка тиков от нового тика заранее неизвестен (dex.integral_tick_steps — тики, а не
//!   слова) — перечитывание только когда обмен хотя бы в одну сторону невозможен из-за края окна — то же правило, что
//!   у математики (math/swap_algebra.rs blocked_by_window: tick < lo или tick ≥ hi; решение 323 к 320), поэтому
//!   «расчёт вернул нули из-за окна» всегда ведёт к перечитыванию. Тик вне [lo; hi] — перечитывать (после чтения он в
//!   окне, иначе загрузка — ошибка, load_algebra_integral.rs). Тик ровно hi (загрузчик это допускает: окно — целые
//!   слова внутри прочитанного отрезка) — перечитывать, только если в карте выше тика меньше dex.integral_tick_steps
//!   тиков: перечитывание идёт вверх от nextTickGlobal (> tick) ровно на dex.integral_tick_steps тиков и все их кладёт
//!   в карту (load_algebra_integral.rs walk — тики вне окна тоже); если столько уже есть — прочитанный отрезок не
//!   вырастет, окно то же (без перечитывания на каждом Swap). После перечитывания у тика hi их не меньше шагов — повтора
//!   нет. Опирается на то, что в списке Integral только тики с liquidityTotal > 0, кроме концов MIN_TICK / MAX_TICK
//!   (TickManagement.sol update/insertTick/removeTick), и что карта Integral событиями не правится (Mint/Burn — Refresh,
//!   events_algebra.rs / events_algebra_integral.rs). Полоса частично вне окна при обмене, возможном в обе стороны, —
//!   частичный итог, как раньше.
//!
//! Кто вызывает: pools/depth.rs (off_map).
//! От чего зависит: ctx.rs, families.rs (Proto), load_algebra.rs (row_range), registry.rs, ticks.rs,
//! math/swap_algebra.rs (blocked_by_window).
//! Настройки: dex.algebra_off_map_reread [bsc_dex] (необязательная, да/нет; не задана или нет — false, как до 320);
//! dex.integral_tick_steps [bsc_dex] (Integral, тик ровно hi; не задана — не перечитывать).
//!
//! Битмап THENA Fusion (решение 462, код AlgebraV1): rowNumber = floor(tick / TICK_SPACING) >> 8 — TickMap.covers при
//! spacing карты = tickSpacing пула считает слово так же (TickMap::word_of); у Integral — слова по 256 сырых тиков
//! (load_algebra_integral.rs window, spacing 1).

use super::super::ctx::Ctx;
use super::super::math::swap_algebra::blocked_by_window;
use super::families::Proto;
use super::load_algebra::row_range;
use super::registry::Entry;
use super::ticks::TickMap;

/// Включена ли проверка выхода за карту у Algebra (dex.algebra_off_map_reread = да). Принимает: окружение.
/// Возвращает: true — да; не задана, нет, не читается — false (поведение до решения 320). Ошибок нет.
pub fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("dex.algebra_off_map_reread").unwrap_or(false)
}

/// Даст ли перечитывание пула Algebra вокруг текущего тика `tick` другое окно карты (см. шапку). Принимает: окружение,
/// запись пула, её карту, текущий тик. Возвращает: true — перечитывать; вид не Algebra / Integral или окно не
/// считается (dex.tick_words не задана) — false. Ошибок нет.
pub fn helps(ctx: &Ctx, e: &Entry, map: &TickMap, tick: i32) -> bool {
    match e.family.proto {
        Proto::Algebra => row_range(ctx, tick, map.spacing).is_ok_and(|w| w != map.words),
        Proto::AlgebraIntegral => integral_helps(ctx, map, tick),
        _ => false,
    }
}

/// Algebra Integral (см. шапку): обмен в какую-то сторону невозможен из-за края окна, и перечитывание вокруг тика
/// `tick` может расширить окно. Принимает: окружение, карту, текущий тик. Возвращает: true — перечитывать; обмен в обе
/// стороны возможен, тик ровно hi и выше него в карте уже не меньше dex.integral_tick_steps тиков, или настройка не
/// задана — false. Ошибок нет.
fn integral_helps(ctx: &Ctx, map: &TickMap, tick: i32) -> bool {
    if !blocked_by_window(map, tick) {
        return false;
    }
    if !map.covers(tick) {
        return true;
    }
    // Тик в окне, но вверх обмен невозможен — тик ровно hi (при spacing 1 covers = окно window_ticks).
    let above = map.ticks.range(tick.saturating_add(1)..).count() as u64;
    ctx.int("dex.integral_tick_steps")
        .is_ok_and(|steps| above < steps.max(1))
}
