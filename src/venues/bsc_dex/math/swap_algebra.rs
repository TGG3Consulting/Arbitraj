//! Файл: venues/bsc_dex/math/swap_algebra.rs
//! Что делает: цикл обмена в пуле Algebra (THENA Fusion и другие форки Algebra V1/V1.9; решения 80, 81) по локальной
//! карте тиков — порт `_calculateSwapAndLock` официального контракта AlgebraPool. Идёт шагами: следующий
//! инициализированный тик в сторону обмена → шаг (math/swap_step::step_v3) → пересечение тика (liquidityDelta).
//! Главное про Algebra (по офдокам, решение 23):
//! (1) Шаг цены (PriceMovementMath.movePriceTowardsTarget) совпадает с Uniswap V3 (SwapMath.computeSwapStep) до
//! последнего целого: та же TokenDeltaMath (= sqrt_price_math::amount0/1_delta) и getNewPrice (= next_from_input/output),
//! та же логика комиссии (единицы 1e-6). Поэтому шаг переиспользуется из math/swap_step::step_v3 — совпадение с
//! Algebra доказано на ОФИЦИАЛЬНЫХ числах test/PriceMovement.spec.ts (tests/bsc_dex_algebra_math.rs). Отдельного
//! модуля шага Algebra не нужно.
//! (2) Комиссия пула у Algebra динамическая (плагин/оракул волатильности пересчитывает её), но ВНУТРИ одного свопа она
//! постоянна: контракт берёт её один раз (globalState.fee, при первом свопе в блоке — _getNewFee) и держит на все шаги.
//! Поэтому для расчёта одного обмена она приходит сюда числом (s.fee) — ровно как fee у V3. Её чтение (globalState.fee)
//! — в загрузчике семейства (фаза B), сверка с живым квотером — решение 73.
//! (3) Отличие от V3, из-за которого нельзя взять math/swap_cl::next_tick: битмап тиков Algebra (TickTable) индексирует
//! СЫРОЙ тик (rowNumber = tick >> 8, bitNumber = tick & 0xFF), БЕЗ деления на tickSpacing (в V3 — compressed =
//! tick / spacing). Поэтому свой поиск следующего тика (next_init по карте) и своё окно прочитанных слов в сырых тиках.
//! Ликвидность одна (u128), reinvestL у Algebra нет (это Elastic).
//! ЭТОТ модуль — только математика (без сети): чтение состояния/тиков, события, квотер, граф — фаза B.
//! Кто вызывает: quote/pool_quote.rs (через фазу B); тесты.
//! От чего зависит: math/swap_step.rs (step_v3), math/tick_math.rs, pools/ticks.rs (TickMap).
//! Настройки: нет (комиссия пула s.fee читается с контракта в фазе B).
//!
//! Источники (официальные, решение 23): cryptoalgebra/AlgebraV1.9 (= версия THENA Fusion, подтвердить адреса в фазе B):
//! цикл свопа `_calculateSwapAndLock` (пересечение тика: zeroToOne вычитает liquidityDelta тика, иначе прибавляет;
//! currentTick = zeroToOne ? nextTick-1 : nextTick) — src/core/contracts/AlgebraPool.sol; поиск следующего тика
//! `nextTickInTheSameRow` (lte = zeroToOne) — src/core/contracts/libraries/TickTable.sol; TickMath (константы и
//! пределы цены MIN_SQRT_RATIO+1 / MAX_SQRT_RATIO-1) совпадает с Uniswap V3 — src/core/contracts/libraries/TickMath.sol.

use alloy_primitives::{I256, U256};

use super::super::pools::ticks::TickMap;
use super::swap_step::step_v3;
use super::tick_math::{MAX_TICK, MIN_TICK, max_sqrt_ratio, min_sqrt_ratio, sqrt_ratio_at_tick};

const OVERFLOW: &str = "переполнение/недобор в математике Algebra";

/// Состояние пула Algebra для расчёта обмена.
pub struct AlgebraState<'a> {
    pub sqrt_price: U256,
    pub tick: i32,
    pub liquidity: u128,
    /// Комиссия пула в пипсах (единицы 1e-6), постоянная на весь своп (globalState.fee; см. шапку, пункт 2).
    pub fee: u32,
    pub ticks: &'a TickMap,
}

/// Итог обмена.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swapped {
    /// Сколько отдаём в пул (с комиссией).
    pub amount_in: U256,
    /// Сколько получаем из пула.
    pub amount_out: U256,
    /// true — вся заданная сумма исполнена в прочитанных пределах карты тиков.
    pub complete: bool,
}

/// Обмен (AlgebraPool.swap). Принимает: состояние, направление (true — token0 → token1), точный вход или выход, сумму.
/// Возвращает: Swapped; Err — там, где контракт откатился бы или карта тиков не сходится.
pub fn swap(
    s: &AlgebraState,
    zero_for_one: bool,
    exact_in: bool,
    amount: U256,
) -> Result<Swapped, &'static str> {
    let limit = if zero_for_one {
        min_sqrt_ratio() + U256::from(1u8)
    } else {
        max_sqrt_ratio() - U256::from(1u8)
    };
    swap_to(s, zero_for_one, exact_in, amount, limit).map(|(r, _)| r)
}

/// Обмен до предела цены `limit` (как swap контракта с limitSqrtPrice): цена дальше предела не идёт, остаток суммы не
/// исполняется. Решение 73 (глубина пула в полосе цены). Принимает: состояние, направление, точный вход или выход,
/// сумму, предел. Возвращает: итог и дошла ли цена до предела. Ошибки: как swap.
pub fn swap_to(
    s: &AlgebraState,
    zero_for_one: bool,
    exact_in: bool,
    amount: U256,
    limit: U256,
) -> Result<(Swapped, bool), &'static str> {
    let (mut sqrt, mut tick, mut liq) = (s.sqrt_price, s.tick, s.liquidity);
    let (mut remaining, mut total_in, mut total_out) = (amount, U256::ZERO, U256::ZERO);
    let (lo, hi) = window_ticks(s.ticks);
    while !remaining.is_zero() && sqrt != limit {
        // Следующий инициализированный тик в сторону обмена (TickTable.nextTickInTheSameRow, lte = zeroToOne).
        // Нет такого в прочитанном окне — идём к краю окна (за ним тики неизвестны: частичный итог).
        let init = next_init(s.ticks, tick, zero_for_one);
        let (bound, is_cross) = match init {
            Some(t) if t >= lo && t <= hi => (t, true),
            _ => (if zero_for_one { lo } else { hi }, false),
        };
        let bound = bound.clamp(MIN_TICK, MAX_TICK);
        let sqrt_bound = sqrt_ratio_at_tick(bound).ok_or("цена тика вне диапазона TickMath")?;
        let target =
            if (zero_for_one && sqrt_bound < limit) || (!zero_for_one && sqrt_bound > limit) {
                limit
            } else {
                sqrt_bound
            };
        let signed = I256::try_from(remaining).map_err(|_| OVERFLOW)?;
        let (new_sqrt, a_in, a_out, fee) = step_v3(
            sqrt,
            target,
            liq,
            if exact_in { signed } else { -signed },
            s.fee,
        )
        .ok_or("шаг Algebra не считается (переполнение или нулевая ликвидность на шаге)")?;
        let start = sqrt;
        sqrt = new_sqrt;
        let spent = a_in.checked_add(fee).ok_or(OVERFLOW)?;
        remaining = remaining
            .checked_sub(if exact_in { spent } else { a_out })
            .ok_or(OVERFLOW)?;
        total_in = total_in.checked_add(spent).ok_or(OVERFLOW)?;
        total_out = total_out.checked_add(a_out).ok_or(OVERFLOW)?;
        if sqrt == sqrt_bound {
            if !is_cross {
                // Дошли до края прочитанного окна — за ним тики неизвестны: итог частичный.
                return Ok((partial(total_in, total_out), false));
            }
            // Пересечение инициализированного тика: liquidity ± liquidityDelta (zeroToOne вычитает, иначе прибавляет —
            // AlgebraPool.sol). У нас net хранится как liquidityDelta тика (как у V3), знак по направлению.
            let net = s
                .ticks
                .ticks
                .get(&bound)
                .ok_or("тик отмечен в окне, но его нет в карте тиков")?
                .net;
            let net = if zero_for_one {
                net.checked_neg().ok_or(OVERFLOW)?
            } else {
                net
            };
            liq = if net >= 0 {
                liq.checked_add(net.unsigned_abs())
            } else {
                liq.checked_sub(net.unsigned_abs())
            }
            .ok_or("ликвидность после пересечения тика вне диапазона (карта тиков не сходится)")?;
            tick = if zero_for_one { bound - 1 } else { bound };
        } else if sqrt != start && !remaining.is_zero() && sqrt != limit {
            // Шаг не дошёл до цели, но сумма не исчерпана — так в контракте не бывает; не гадаем.
            return Err("шаг Algebra остановился до тика при остатке суммы");
        }
    }
    Ok((
        Swapped {
            amount_in: total_in,
            amount_out: total_out,
            complete: remaining.is_zero(),
        },
        sqrt == limit,
    ))
}

/// Следующий инициализированный тик карты в сторону обмена (TickTable.nextTickInTheSameRow): zeroToOne (lte) — не
/// больше `cur`; иначе — строго больше `cur`. None — в прочитанных тиках такого нет. Ошибок нет.
fn next_init(t: &TickMap, cur: i32, zero_for_one: bool) -> Option<i32> {
    if zero_for_one {
        t.ticks.range(..=cur).next_back().map(|(k, _)| *k)
    } else {
        t.ticks.range((cur + 1)..).next().map(|(k, _)| *k)
    }
}

/// Диапазон СЫРЫХ тиков, для которого прочитаны все инициализированные тики. Битмап Algebra индексирует сырой тик
/// (TickTable: rowNumber = tick >> 8), поэтому слово w покрывает тики [w·256, w·256+255] — БЕЗ умножения на spacing
/// (в отличие от V3/Elastic). Загрузчик фазы B заполняет words сырыми позициями слов (или (i16::MIN, i16::MAX),
/// когда читает все тики сразу). Ошибок нет.
fn window_ticks(t: &TickMap) -> (i32, i32) {
    let lo = t.words.0 as i32 * 256;
    let hi = t.words.1 as i32 * 256 + 255;
    (lo.clamp(MIN_TICK, MAX_TICK), hi.clamp(MIN_TICK, MAX_TICK))
}

/// Частичный итог (цена вышла за прочитанное окно тиков): complete = false. Ошибок нет.
fn partial(total_in: U256, total_out: U256) -> Swapped {
    Swapped {
        amount_in: total_in,
        amount_out: total_out,
        complete: false,
    }
}
