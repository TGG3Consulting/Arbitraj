//! Файл: venues/bsc_dex/quote/solidly_depth.rs
//! Что делает: глубина СТАБИЛЬНОГО пула Solidly (ve(3,3)-форки: Topaz v2, THENA V1; решения 80, 85) по направлению в
//! полосе цены (решение 73). У стабильной кривой `x³y+xy³=k` цена границы полосы не выражается одним пределом (нет
//! тиков), поэтому глубина считается по СРЕДНЕЙ цене исполнения: сколько встречного токена пул примет (покупка) или
//! отдаст (продажа), пока средняя цена монеты не отойдёт от текущей (маржинальной) цены на полосу b. Расчёт — по своей
//! официальной математике (math/swap_solidly, HEAD b3065d8), БЕЗ комиссии пула (чистый объём пула в полосе, решение 73
//! — комиссия только в котировке/прибыли). Монотонность кривой → расширение вверх + двоичный поиск границы (как у
//! DODO PMM, quote/pmm_depth.rs). ВОЛАТИЛЬНЫЙ пул Solidly — обычная V2-формула (quote/depth_band.rs, ветка V2), сюда
//! не идёт.
//! Кто вызывает: quote/depth_band.rs (ветка Solidly, stable=true).
//! От чего зависит: math/swap_solidly.rs, math/full_math.rs.
//! Настройки: полосу даёт вызывающий (dex.depth_band_pct).
//!
//! ЗАМЕЧАНИЕ (на подтверждение Тиграну, как для PMM): решение 73 определило метод полосы для V2/V3/CL/Bin, но не для
//! стабильной кривой Solidly — здесь полоса трактуется как отклонение СРЕДНЕЙ цены исполнения от маржинальной (оценка
//! консервативная: к границе полосы цена degradирует быстрее средней, поэтому глубина не завышается). Иной критерий —
//! вопрос к Тиграну.

use alloy_primitives::U256;

use super::super::math::full_math::mul_div;
use super::super::math::swap_solidly;

/// Максимум удвоений при поиске верхней границы объёма (цена кривой расходится — граница всегда находится). Предел
/// итераций, не порог торговли.
const MAX_DOUBLE: u32 = 192;
/// Шагов двоичного поиска границы полосы.
const BISECT: u32 = 64;

/// 1e18 — единица нормализации фикс-точки (как в контракте Solidly). Не настройка.
fn e18() -> U256 {
    U256::from(10u8).pow(U256::from(18u8))
}

/// Резервы одного пула в нормализации к 1e18 по своим decimals. amount·1e18/dec. None — переполнение/деление на ноль.
fn norm(amount: U256, dec: U256) -> Option<U256> {
    if dec.is_zero() {
        return None;
    }
    Some(amount.checked_mul(e18())? / dec)
}

/// Маржинальная цена монеты во встречном токене (1e18), из НОРМАЛИЗОВАННЫХ резервов: монета xc, встречный yu.
/// Для инварианта f=x³y+xy³ цена x в y = f_x/f_y = (3·xc²·yu + yu³)/(xc³ + 3·xc·yu²). None — переполнение/нулевой
/// знаменатель. Ошибок нет.
fn price_norm(xc: U256, yu: U256) -> Option<U256> {
    let three = U256::from(3u8);
    let xc2 = xc.checked_mul(xc)?;
    let yu2 = yu.checked_mul(yu)?;
    let num = three
        .checked_mul(xc2)?
        .checked_mul(yu)?
        .checked_add(yu2.checked_mul(yu)?)?;
    let den = xc2
        .checked_mul(xc)?
        .checked_add(three.checked_mul(xc)?.checked_mul(yu2)?)?;
    if den.is_zero() {
        return None;
    }
    mul_div(num, e18(), den)
}

/// Глубина одной стороны стабильного пула Solidly в полосе b (доля 0..1). Принимает: резервы token0/token1 и их
/// масштабы decimals (10^decimals), монета ли это token0, полосу b, направление (buy — покупка монеты: встречный
/// токен в пул). Возвращает: (сумма во ВСТРЕЧНОМ токене, полоса посчитана целиком). None — нет цены (пустая сторона)
/// или расчёт невозможен. Комиссии нет (чистый объём, решение 73).
pub fn side(
    reserve0: U256,
    reserve1: U256,
    decimals0: U256,
    decimals1: U256,
    coin_is_token0: bool,
    b: f64,
    buy: bool,
) -> Option<(U256, bool)> {
    if !(b > 0.0 && b < 1.0) {
        return None;
    }
    // Резервы и decimals монеты и встречного токена (в единицах токенов и как 10^decimals).
    let (coin_res, coin_dec, cnt_res, cnt_dec) = if coin_is_token0 {
        (reserve0, decimals0, reserve1, decimals1)
    } else {
        (reserve1, decimals1, reserve0, decimals0)
    };
    // Маржинальная цена монеты (встречный за монету, 1e18) из нормализованных резервов.
    let xc = norm(coin_res, coin_dec)?;
    let yu = norm(cnt_res, cnt_dec)?;
    let price = price_norm(xc, yu)?;
    if price.is_zero() {
        return None;
    }
    // Граница полосы по средней цене: покупка — вверх ×(1+b), продажа — вниз ×(1−b). Полоса в базисных пунктах.
    let bp = (b * 10_000.0) as u64;
    let limit = if buy {
        mul_div(price, U256::from(10_000 + bp), U256::from(10_000u64))?
    } else {
        mul_div(price, U256::from(10_000 - bp), U256::from(10_000u64))?
    };
    // Средняя цена монеты (встречный/монета, 1e18) на входе x; в полосе ли (покупка: ≤ limit; продажа: ≥).
    let within = |x: U256| -> Option<bool> {
        let (cnt, coin) = amounts(coin_res, coin_dec, cnt_res, cnt_dec, buy, x)?;
        let coin_n = norm(coin, coin_dec)?;
        if coin_n.is_zero() {
            return Some(false);
        }
        let cnt_n = norm(cnt, cnt_dec)?;
        let avg = mul_div(cnt_n, e18(), coin_n)?;
        Some(if buy { avg <= limit } else { avg >= limit })
    };
    // Полоса уже на минимальном объёме не держится — глубины нет.
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
        return Some((
            at_counter(coin_res, coin_dec, cnt_res, cnt_dec, buy, lo)?,
            false,
        ));
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
    Some((
        at_counter(coin_res, coin_dec, cnt_res, cnt_dec, buy, lo)?,
        true,
    ))
}

/// Встречный и монетный объёмы свопа на входе x (без комиссии): покупка — вход встречный (cnt=x, монета=выход);
/// продажа — вход монета (монета=x, cnt=выход). None — откат математики. Ошибок нет.
fn amounts(
    coin_res: U256,
    coin_dec: U256,
    cnt_res: U256,
    cnt_dec: U256,
    buy: bool,
    x: U256,
) -> Option<(U256, U256)> {
    let zero = U256::ZERO;
    if buy {
        // Встречный токен в пул → монета из пула.
        let out = swap_solidly::get_amount_out(x, cnt_res, coin_res, cnt_dec, coin_dec, zero)?;
        Some((x, out))
    } else {
        // Монета в пул → встречный токен из пула.
        let out = swap_solidly::get_amount_out(x, coin_res, cnt_res, coin_dec, cnt_dec, zero)?;
        Some((out, x))
    }
}

/// Сумма во ВСТРЕЧНОМ токене на входе x (покупка — x, продажа — выход). None — откат. Ошибок нет.
fn at_counter(
    coin_res: U256,
    coin_dec: U256,
    cnt_res: U256,
    cnt_dec: U256,
    buy: bool,
    x: U256,
) -> Option<U256> {
    amounts(coin_res, coin_dec, cnt_res, cnt_dec, buy, x).map(|(cnt, _)| cnt)
}
