//! Файл: venues/bsc_dex/math/swap_algebra.rs
//! Что делает: цикл обмена в пуле Algebra (THENA Fusion — Algebra V1, Camelot V3 — Algebra V1.9, Algebra Integral;
//! решения 80, 81) по локальной карте тиков — порт `_calculateSwapAndLock` официального контракта AlgebraPool. Идёт
//! шагами: следующий инициализированный тик в сторону обмена → шаг (math/swap_step::step_v3) → пересечение тика
//! (liquidityDelta).
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
//! (3) Окно карты: у Integral (Linked) — слова по 256 СЫРЫХ тиков (load_algebra_integral.rs, window_ticks); у THENA
//! Fusion (Row) — сжатые строки по 256·spacing тиков (решение 463, пункт 5; до 463 здесь было ошибочно «сырой битмап»
//! и для THENA Fusion); у Algebra V1.9 Camelot V3 (Row) — сырые строки (spacing = 1, решение 461).
//! Ликвидность одна (u128), reinvestL у Algebra нет (это Elastic).
//! (4) Решение 323: текущий тик вне окна, нужного обмену (вниз — lo ≤ tick ≤ hi, вверх — lo − 1 ≤ tick < hi, как V3
//! swap_cl::next_tick), — неполный итог с нулями (complete = false), как у V3, а не шаг к краю окна по другую сторону
//! цены. Общая функция для THENA Fusion (V1), Algebra V1.9 и Algebra Integral, котировки (quote/pool_quote.rs) и
//! глубины (quote/depth_band_algebra.rs); то же правило — blocked_by_window для перечитывания
//! (pools/off_map_algebra.rs).
//! (5) Решение 463 (запись своего узла BSC 20.11, THENA Fusion): цель шага у пула с битмапом (режим NextTick::Row) —
//! НЕ следующий инициализированный тик, а порт TickTable.nextTickInTheSameRow официального кода пула: инициализированный
//! тик в той же строке битмапа или, если его нет, край строки (неинициализированный — шаг до него без пересечения;
//! каждый шаг округляет вход вверх, выход вниз — прыжок через край строки дал бы другой итог). Строка — 256 СЖАТЫХ
//! тиков: THENA Fusion — исходник Sourcify THENA (сеть 56, пул 0x1b9a…ef7d и деплоер 0xc89F…7358 — полное совпадение
//! байткода; линия AlgebraV1: TickTable, TICK_SPACING и цикл совпадают с cryptoalgebra/AlgebraV1 master, комиссия
//! сообщества — uint16): TickTable.sol toggleTick `tick /= Constants.TICK_SPACING`, nextTickInTheSameRow сжимает тик
//! (sdiv с округлением к −∞) и возвращает uncompressAndBoundTick (× TICK_SPACING, в пределах MIN_TICK…MAX_TICK);
//! цикл — AlgebraPool.sol _calculateSwapAndLock (пересечение только у initialized, currentTick = zeroToOne ?
//! nextTick − 1 : nextTick). Шаг — TickMap.spacing (tickSpacing() пула, load_algebra.rs). До 463 бот читал битмап
//! THENA Fusion как СЫРОЙ (слово тика tick >> 8) — у 7 из 8 записанных пулов карта была пустой, обмен шёл по одной
//! ликвидности без пересечений (на записи: до +0,96% к выходу квотера). Algebra Integral (связный список тиков) —
//! режим NextTick::Linked: сразу следующий инициализированный тик, поведение прежнее.
//! Слияние BSC → Arbitrum (решения 461 и 463): Algebra V1.9 (Camelot V3 — algebra_dir_fee; cryptoalgebra/AlgebraV1.9
//! a45417ed libraries/TickTable.sol:65-108) — тот же режим Row, но битмап СЫРОЙ (строка = 256 сырых тиков):
//! загрузчик ставит TickMap.spacing = 1 (pools/load_algebra.rs), и in_same_row сводится к сырой строке. Запись
//! Arbitrum (пул 0x7cccba…, 34138539 token1): прыжок через край строки давал выход больше пула.
//! ЭТОТ модуль — только математика (без сети): чтение состояния/тиков, события, квотер, граф — фаза B.
//! Кто вызывает: quote/pool_quote.rs (котировка), quote/depth_band_algebra.rs (глубина, swap_to),
//! pools/off_map_algebra.rs (blocked_by_window); тесты.
//! От чего зависит: math/swap_step.rs (step_v3), math/tick_math.rs, pools/ticks.rs (TickMap).
//! Настройки: нет (комиссия пула s.fee читается с контракта в фазе B).
//!
//! Источники (официальные, решение 23; версия THENA Fusion — исходник Sourcify THENA, линия AlgebraV1 (TickTable, TICK_SPACING и цикл = master; комиссия сообщества — uint16), решение 463): cryptoalgebra/AlgebraV1.9 и V1:
//! цикл свопа `_calculateSwapAndLock` (пересечение тика: zeroToOne вычитает liquidityDelta тика, иначе прибавляет;
//! currentTick = zeroToOne ? nextTick-1 : nextTick) — src/core/contracts/AlgebraPool.sol; поиск следующего тика
//! `nextTickInTheSameRow` (lte = zeroToOne) — src/core/contracts/libraries/TickTable.sol; TickMath (константы и
//! пределы цены MIN_SQRT_RATIO+1 / MAX_SQRT_RATIO-1) совпадает с Uniswap V3 — src/core/contracts/libraries/TickMath.sol.

use alloy_primitives::{I256, U256};

use super::super::pools::ticks::TickMap;
use super::swap_step::step_v3;
use super::tick_math::{MAX_TICK, MIN_TICK, max_sqrt_ratio, min_sqrt_ratio, sqrt_ratio_at_tick};

const OVERFLOW: &str = "переполнение/недобор в математике Algebra";

/// Как цикл обмена выбирает цель шага (шапка, пункт 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextTick {
    /// Битмап строками по 256 сжатых тиков (шаг — TickMap.spacing): TickTable.nextTickInTheSameRow —
    /// инициализированный тик строки или край строки. THENA Fusion (код AlgebraV1, решение 463: spacing =
    /// tickSpacing пула) и Algebra V1.9 Camelot V3 (algebra_dir_fee, решение 461: сырой битмап — spacing = 1).
    Row,
    /// Algebra Integral: связный список тиков — сразу следующий инициализированный тик (поведение до 463).
    Linked,
}

/// Состояние пула Algebra для расчёта обмена.
pub struct AlgebraState<'a> {
    pub sqrt_price: U256,
    pub tick: i32,
    pub liquidity: u128,
    /// Комиссия пула в пипсах (единицы 1e-6), постоянная на весь своп (globalState.fee; см. шапку, пункт 2).
    pub fee: u32,
    pub ticks: &'a TickMap,
    /// Выбор цели шага по коду пула (шапка, пункт 5).
    pub next: NextTick,
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
    // Решение 323: текущий тик вне прочитанного окна (после Swap до перечитывания) — честный неполный итог с нулями,
    // как у V3 (math/swap_cl.rs walk: next_tick → None). Иначе граница — край окна по другую сторону цены (шаг в
    // обратную сторону, выход в единицах другого токена) или постоянная ликвидность на непрочитанном отрезке.
    if s.next == NextTick::Linked
        && !remaining.is_zero()
        && sqrt != limit
        && !starts_in_window(tick, lo, hi, zero_for_one)
    {
        return Ok((partial(U256::ZERO, U256::ZERO), false));
    }
    while !remaining.is_zero() && sqrt != limit {
        let (bound, is_cross) = match s.next {
            // Строка, в которой ищется цель, должна быть прочитана; иначе частичный итог (за ней тики неизвестны).
            NextTick::Row => match in_same_row(s.ticks, tick, zero_for_one) {
                Some(x) => x,
                None => return Ok((partial(total_in, total_out), false)),
            },
            // Integral: следующий инициализированный тик; нет его в прочитанном окне — идём к краю окна (за ним тики
            // неизвестны: частичный итог).
            NextTick::Linked => match next_init(s.ticks, tick, zero_for_one) {
                Some(t) if t >= lo && t <= hi => (t, true),
                _ => (if zero_for_one { lo } else { hi }, false),
            },
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
                if s.next == NextTick::Row {
                    // Край строки без инициализированного тика: ликвидность не меняется, currentTick = zeroToOne ?
                    // nextTick − 1 : nextTick (AlgebraPool.sol _calculateSwapAndLock).
                    tick = if zero_for_one { bound - 1 } else { bound };
                    continue;
                }
                // Integral: дошли до края прочитанного окна — за ним тики неизвестны: итог частичный.
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

/// Цель шага режима Row — порт TickTable.nextTickInTheSameRow (исходник Sourcify THENA, libraries/TickTable.sol — как в
/// cryptoalgebra/AlgebraV1 master):
/// c = floor(cur / spacing); lte (zeroToOne) — строка c >> 8, наибольший инициализированный сжатый тик не выше c, иначе
/// начало строки; иначе — строка c + 1, наименьший инициализированный сжатый тик не ниже c + 1, иначе конец строки.
/// Итог — × spacing в пределах [MIN_TICK; MAX_TICK] (uncompressAndBoundTick). Карта хранит несжатые тики (кратные
/// spacing). Принимает: карту, текущий тик, направление. Возвращает: (тик, инициализирован ли); None — строка не
/// прочитана (вне TickMap.words). Ошибок нет.
fn in_same_row(t: &TickMap, cur: i32, zero_for_one: bool) -> Option<(i32, bool)> {
    let sp = t.spacing.max(1);
    let c = cur.div_euclid(sp);
    let (c, row_lo, row_hi) = if zero_for_one {
        (c, c - c.rem_euclid(256), c)
    } else {
        let c1 = c + 1;
        (c1, c1, c1 - c1.rem_euclid(256) + 255)
    };
    let row = c.div_euclid(256);
    if row < i32::from(t.words.0) || row > i32::from(t.words.1) {
        return None;
    }
    let range = row_lo.saturating_mul(sp)..=row_hi.saturating_mul(sp);
    let found = if zero_for_one {
        t.ticks.range(range).next_back()
    } else {
        t.ticks.range(range).next()
    };
    let (tick, init) = match found {
        Some((k, _)) => (*k, true),
        None => (
            (if zero_for_one { row_lo } else { row_hi }).saturating_mul(sp),
            false,
        ),
    };
    Some((tick.clamp(MIN_TICK, MAX_TICK), init))
}

/// Следующий инициализированный тик карты в сторону обмена (Algebra Integral, режим Linked): zeroToOne (lte) — не
/// больше `cur`; иначе — строго больше `cur`. None — в прочитанных тиках такого нет. Ошибок нет.
fn next_init(t: &TickMap, cur: i32, zero_for_one: bool) -> Option<i32> {
    if zero_for_one {
        t.ticks.range(..=cur).next_back().map(|(k, _)| *k)
    } else {
        t.ticks.range((cur + 1)..).next().map(|(k, _)| *k)
    }
}

/// Диапазон тиков окна карты в режиме Linked (Algebra Integral): слово w покрывает тики [w·256, w·256+255] — карта
/// Integral строится по 256 сырых тиков (load_algebra_integral.rs; (i16::MIN, i16::MAX), когда прочитано всё). THENA
/// Fusion (режим Row, решение 463) этим окном не пользуется: его окно — сжатые строки TickMap.words при TickMap.spacing
/// (in_same_row). Ошибок нет.
fn window_ticks(t: &TickMap) -> (i32, i32) {
    let lo = t.words.0 as i32 * 256;
    let hi = t.words.1 as i32 * 256 + 255;
    (lo.clamp(MIN_TICK, MAX_TICK), hi.clamp(MIN_TICK, MAX_TICK))
}

/// Решение 323: можно ли начинать обмен от тика `tick` в окне [lo; hi] — как у V3 (swap_cl::next_tick: слово нужной
/// позиции не прочитано — None; вниз позиция compressed, вверх — compressed + 1). Вниз (lte) нужен сам тик:
/// lo ≤ tick ≤ hi. Вверх ищется тик строго выше текущего — нужна позиция tick + 1: lo − 1 ≤ tick < hi. При tick = hi
/// граница hi лежит НИЖЕ цены (шаг пошёл бы вниз). При tick = lo − 1 цена в [цена(lo − 1); цена(lo)): между ней и lo
/// инициализированных тиков нет, текущая ликвидность — ликвидность этого отрезка, next_init (range(lo..)) находит
/// первый тик карты не ниже lo, включая сам lo, — расчёт верен. lo не ниже MIN_TICK (window_ticks) — lo − 1 без
/// переполнения. Пустое окно (lo > hi) — false в обе стороны. Ошибок нет.
fn starts_in_window(tick: i32, lo: i32, hi: i32, zero_for_one: bool) -> bool {
    if zero_for_one {
        (lo..=hi).contains(&tick)
    } else {
        (lo.saturating_sub(1)..hi).contains(&tick)
    }
}

/// Решение 323 к 320: обмен хотя бы в одну сторону от тика `tick` невозможен из-за края прочитанного окна карты `t`
/// (то же правило starts_in_window, что у swap_to, — «расчёт вернул нули из-за окна» ⇔ true). Итог: tick < lo или
/// tick ≥ hi. Кто вызывает: pools/off_map_algebra.rs (перечитывание Algebra Integral). Ошибок нет.
pub fn blocked_by_window(t: &TickMap, tick: i32) -> bool {
    let (lo, hi) = window_ticks(t);
    !starts_in_window(tick, lo, hi, true) || !starts_in_window(tick, lo, hi, false)
}

/// Частичный итог (цена вышла за прочитанное окно тиков): complete = false. Ошибок нет.
fn partial(total_in: U256, total_out: U256) -> Swapped {
    Swapped {
        amount_in: total_in,
        amount_out: total_out,
        complete: false,
    }
}
