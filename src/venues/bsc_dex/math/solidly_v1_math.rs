//! Файл: venues/bsc_dex/math/solidly_v1_math.rs
//! Что делает: математика пула Solidly V1 — THENA V1 (решение 122, часть F) — порт `getAmountOut` / `_getAmountOut` /
//! `_f` / `_get_y` официального Pair.sol на целых числах U256 с теми же округлениями. Стабильный пул: инвариант
//! `x³y + xy³ = k`, но форма `_f` и цикл `_get_y` ДРУГИЕ, чем у Velodrome (math/solidly_math.rs): `_f` считается как
//! `x0·(y³) + (x0³)·y` с делениями на 1e18 по ходу, а `_get_y` — простой Ньютон до |Δy| ≤ 1 за 255 раундов и БЕЗ отката
//! при несходимости (возвращает последнее y). Инвариант до свопа `_k` — та же форма `_a·_b`, что у Velodrome
//! (swap_solidly::k). Волатильный пул — `amountIn·reserveB / (reserveA + amountIn)` после вычета комиссии из входа
//! (одинаково у THENA V1 и Velodrome — `volatile_amount_out`).
//! Кто вызывает: quote/pool_quote.rs (ветка Solidly по виду пула); тесты.
//! От чего зависит: math/solidly_math.rs (d, e18, fee_denominator), math/swap_solidly.rs (k).
//! Настройки: нет (резервы, decimals, комиссия — с контракта пула и фабрики, load_solidly.rs).
//!
//! Источник (официальный, решение 23): ThenafiBNB/THENA-Contracts, HEAD f5bc742e1ff16c8be308a271a6a4a0e8f9e06453,
//! contracts/Pair.sol (pragma 0.8.13): `_f` L427-L429, `_d` L431-L433, `_get_y` L435-L457, `getAmountOut` L459-L463,
//! `_getAmountOut` L465-L478, `_k` L480-L490. Solidity 0.8 — арифметика checked: переполнение, недобор (`y - dy`) и
//! деление на ноль (`_d = 0`) откатываются → здесь `None`.

use alloy_primitives::U256;

use super::solidly_math::{d, e18, fee_denominator};
use super::swap_solidly::k;

/// Pair._f (THENA V1): `x0·(y·y/1e18·y/1e18)/1e18 + (x0·x0/1e18·x0/1e18)·y/1e18`, порядок операций как в контракте.
/// Принимает: x0, y (нормализованы к 1e18). Возвращает: f. None — переполнение (checked `*`/`+` откатился бы).
pub fn f(x0: U256, y: U256) -> Option<U256> {
    let one = e18();
    let y3 = (y.checked_mul(y)? / one).checked_mul(y)? / one;
    let left = x0.checked_mul(y3)? / one;
    let x3 = (x0.checked_mul(x0)? / one).checked_mul(x0)? / one;
    let right = x3.checked_mul(y)? / one;
    left.checked_add(right)
}

/// Pair._get_y (THENA V1): Ньютон по y до 255 раундов; шаг `dy = |xy − f|·1e18 / _d`; выход, как только |y − y_prev| ≤ 1;
/// после 255 раундов — последнее y (отката нет). Принимает: x0 — новый нормализованный баланс исходного токена, xy —
/// инвариант до свопа, y — стартовое приближение (нормализованный баланс встречного токена). Возвращает: новое y.
/// None — `_d = 0` (деление на ноль), недобор `y − dy` или переполнение (в контракте — откат).
pub fn get_y(x0: U256, xy: U256, mut y: U256) -> Option<U256> {
    let one = e18();
    let unit = U256::from(1u8);
    for _ in 0..255u16 {
        let y_prev = y;
        let kk = f(x0, y)?;
        let dd = d(x0, y)?;
        if dd.is_zero() {
            return None;
        }
        if kk < xy {
            let dy = (xy - kk).checked_mul(one)? / dd;
            y = y.checked_add(dy)?;
        } else {
            let dy = (kk - xy).checked_mul(one)? / dd;
            y = y.checked_sub(dy)?;
        }
        let step = if y > y_prev { y - y_prev } else { y_prev - y };
        if step <= unit {
            return Some(y);
        }
    }
    Some(y)
}

/// Pair.getAmountOut (THENA V1, ветка stable): вход минус `amountIn·fee/10000`, затем `_getAmountOut`: xy = _k по
/// исходным резервам, нормализация резервов и входа к 1e18, `y = reserveB − _get_y(amountIn + reserveA, xy, reserveB)`,
/// выход `y·dec_out/1e18`. Принимает: вход, резервы входа/выхода, 10^decimals входа/выхода, комиссию (числитель над
/// 10000 — getFee(stable) фабрики). Возвращает: выход. None — decimals = 0, недобор/переполнение (откат контракта).
pub fn stable_amount_out(
    amount_in: U256,
    reserve_in: U256,
    reserve_out: U256,
    decimals_in: U256,
    decimals_out: U256,
    fee: U256,
) -> Option<U256> {
    if decimals_in.is_zero() || decimals_out.is_zero() {
        return None;
    }
    let one = e18();
    let amount_in = amount_in.checked_sub(amount_in.checked_mul(fee)? / fee_denominator())?;
    let xy = k(reserve_in, reserve_out, decimals_in, decimals_out)?;
    let res_in_n = reserve_in.checked_mul(one)? / decimals_in;
    let res_out_n = reserve_out.checked_mul(one)? / decimals_out;
    let in_n = amount_in.checked_mul(one)? / decimals_in;
    let new_y = get_y(in_n.checked_add(res_in_n)?, xy, res_out_n)?;
    let y = res_out_n.checked_sub(new_y)?;
    Some(y.checked_mul(decimals_out)? / one)
}

/// Pair.getAmountOut, ветка volatile (одинакова у THENA V1 Pair.sol L474-L476 и Velodrome Pool.sol `_getAmountOut`):
/// вход минус `amountIn·fee/10000` (целое деление — как в контракте), выход `amountIn·reserveB / (reserveA + amountIn)`.
/// Принимает: вход, резервы входа/выхода, комиссию (числитель над 10000). Возвращает: выход. None — переполнение,
/// деление на ноль (оба резерва и вход нулевые), комиссия больше входа (недобор).
pub fn volatile_amount_out(
    amount_in: U256,
    reserve_in: U256,
    reserve_out: U256,
    fee: U256,
) -> Option<U256> {
    let amount_in = amount_in.checked_sub(amount_in.checked_mul(fee)? / fee_denominator())?;
    let den = reserve_in.checked_add(amount_in)?;
    if den.is_zero() {
        return None;
    }
    Some(amount_in.checked_mul(reserve_out)? / den)
}
