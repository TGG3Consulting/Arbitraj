//! Файл: venues/bsc_dex/quote/pmm_depth.rs
//! Что делает: глубина пула DODO PMM по направлению в полосе цены (решение 73) — у кривой PMM нет тиков/бинов, цена
//! границы полосы не выражается одним пределом, поэтому глубина считается по СРЕДНЕЙ цене исполнения: сколько
//! встречного токена пул примет (покупка) или отдаст (продажа), пока средняя цена монеты не отойдёт от текущей
//! средней цены (getMidPrice) на полосу b. Поиск — по своей официальной математике (math/swap_dodo, фаза 86a), БЕЗ
//! комиссии пула (как у прочих видов: чистый объём пула в полосе, решение 73). Монотонность (чем больше объём — тем
//! дальше средняя цена) → расширение вверх + двоичный поиск границы.
//! Кто вызывает: quote/depth_band.rs (ветка Pmm).
//! От чего зависит: math/swap_dodo.rs, math/dodo_math.rs, math/full_math.rs.
//! Настройки: полосу даёт вызывающий (dex.depth_band_pct).
//!
//! ЗАМЕЧАНИЕ (на подтверждение Тиграну): решение 73 определило метод полосы для V2/V3/CL/Bin, но не для PMM —
//! здесь полоса трактуется как отклонение СРЕДНЕЙ цены исполнения (оценка консервативная: цена к границе полосы
//! degradирует быстрее средней, поэтому глубина не завышается). Иной критерий для PMM — вопрос к Тиграну (86b).

use alloy_primitives::U256;

use super::super::math::dodo_math::one;
use super::super::math::full_math::mul_div;
use super::super::math::swap_dodo::{self, PmmState, get_mid_price, reciprocal_floor};

/// Максимум удвоений при поиске верхней границы объёма (цена PMM расходится — граница всегда находится). Это предел
/// итераций, не порог торговли.
const MAX_DOUBLE: u32 = 192;
/// Шагов двоичного поиска границы полосы.
const BISECT: u32 = 64;

/// Глубина одной стороны PMM в полосе b (доля 0..1). Принимает: состояние кривой, монета ли это базовый токен пула,
/// полосу b, направление (buy — покупка монеты, встречный токен в пул). Возвращает: (сумма во ВСТРЕЧНОМ токене,
/// полоса посчитана целиком). None — нет средней цены (пустая сторона) или расчёт невозможен.
pub fn side(state: &PmmState, coin_is_base: bool, b: f64, buy: bool) -> Option<(U256, bool)> {
    if !(b > 0.0 && b < 1.0) {
        return None;
    }
    let mid = get_mid_price(state)?; // quote за base, 18 знаков
    if mid.is_zero() {
        return None;
    }
    // Цена монеты во встречном токене (18 знаков): монета — база → mid; монета — котируемая → 1/mid.
    let mid_coin = if coin_is_base {
        mid
    } else {
        reciprocal_floor(mid)?
    };
    // Граница полосы по средней цене монеты: покупка — вверх (×(1+b)), продажа — вниз (×(1−b)). Полоса в б.п.
    let bp = (b * 10_000.0) as u64;
    let limit = if buy {
        mul_div(mid_coin, U256::from(10_000 + bp), U256::from(10_000u64))?
    } else {
        mul_div(mid_coin, U256::from(10_000 - bp), U256::from(10_000u64))?
    };
    // Вход свопа: покупка — встречный токен; продажа — монета. База ли это (→ sellBase, иначе sellQuote):
    let input_is_base = if buy { !coin_is_base } else { coin_is_base };
    // Средняя цена монеты (встречный/монета, 18 знаков) на объёме входа x; в полосе ли (покупка: ≤ limit; продажа: ≥).
    let within = |x: U256| -> Option<bool> {
        let (counter, coin) = amounts(state, input_is_base, buy, x)?;
        if coin.is_zero() {
            return Some(false);
        }
        let avg = mul_div(counter, one(), coin)?; // встречный за монету, 18 знаков
        Some(if buy { avg <= limit } else { avg >= limit })
    };
    // Полоса уже на нулевом объёме не держится — глубины нет.
    if !within(U256::from(1u8)).unwrap_or(false) {
        return Some((U256::ZERO, true));
    }
    // Расширение: удваиваем, пока в полосе.
    let mut lo = U256::from(1u8);
    let mut hi = U256::from(1u8);
    let mut found_hi = false;
    for _ in 0..MAX_DOUBLE {
        let next = hi.checked_mul(U256::from(2u8))?;
        if within(next).unwrap_or(false) {
            lo = next;
            hi = next;
        } else {
            hi = next;
            found_hi = true;
            break;
        }
    }
    if !found_hi {
        // Не нашли границу за предел итераций — берём достигнутый объём как глубину (full=false).
        return Some((at_counter(state, input_is_base, buy, lo)?, false));
    }
    // Двоичный поиск границы в (lo, hi].
    for _ in 0..BISECT {
        let mid_x = lo.checked_add(hi.checked_sub(lo)? / U256::from(2u8))?;
        if mid_x == lo {
            break;
        }
        if within(mid_x).unwrap_or(false) {
            lo = mid_x;
        } else {
            hi = mid_x;
        }
    }
    Some((at_counter(state, input_is_base, buy, lo)?, true))
}

/// Встречный и монетный объёмы свопа на входе x: покупка — вход встречный (counter=x, монета=выход);
/// продажа — вход монета (монета=x, counter=выход). Без комиссии пула (чистый объём). None — откат математики.
fn amounts(state: &PmmState, input_is_base: bool, buy: bool, x: U256) -> Option<(U256, U256)> {
    let out = if input_is_base {
        swap_dodo::sell_base_token(state, x)?.0
    } else {
        swap_dodo::sell_quote_token(state, x)?.0
    };
    Some(if buy { (x, out) } else { (out, x) })
}

/// Сумма во ВСТРЕЧНОМ токене на входе x (покупка — x, продажа — выход). None — откат. Ошибок нет.
fn at_counter(state: &PmmState, input_is_base: bool, buy: bool, x: U256) -> Option<U256> {
    amounts(state, input_is_base, buy, x).map(|(counter, _)| counter)
}
