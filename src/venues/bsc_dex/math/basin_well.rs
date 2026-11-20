//! Файл: venues/bsc_dex/math/basin_well.rs
//! Что делает: решение 485 — обмен в скважине Basin один-в-один с контрактом: выход = резерв_j − calcReserve(функция
//! скважины, резервы с прибавленным входом, j, totalSupply) (Well.sol _swapFrom L243-L253, getSwapOut L266-L279);
//! функции скважины ConstantProduct2 (calcReserve с округлением вверх, calcLpTokenSupply — sqrt) и Stable2 (итерации
//! calcReserve и calcLpTokenSupply с точностью 1, масштаб резервов к 18 знакам). Своей комиссии у скважины нет.
//! Переполнение, деление на ноль, неуспех итерации (у контракта — откат) — None.
//! Кто вызывает: quote/pool_quote.rs, quote/basin_depth.rs, pools/verify_basin_judge.rs (calc_reserve — сверка версии
//! функции и A), tests/basin_well_math.rs (calcLpTokenSupply — официальные векторы).
//! От чего зависит: core/types (BasinWellData, BasinFunction), alloy-primitives.
//! Настройки: нет.
//!
//! Источник (официальный): BeanstalkFarms/Basin@ecf69233 — src/Well.sol, src/functions/ConstantProduct2.sol,
//! src/functions/Stable2.sol, src/libraries/LibMath.sol L14-L17 (roundUpDiv). Развёрнутые на Arbitrum
//! ConstantProduct2 0xBA15…72b4 и Stable2 0xba15…1CBC — те же тексты (Sourcify 42161, SHA-256 исходника совпал с
//! ecf69233). Well Implementation 0xBA51…a1d9 и Aquifer — Well.sol 01723b6 (= 67abd9c): отличия от ecf69233 —
//! @author, init без `virtual`, `if (…) revert InvalidReserves();` в _setReserves одной строкой (та же проверка) и
//! переносы строк; Upgradable Well 0xBA51…e50B — Well.sol 5d12098 и WellUpgradeable.sol 6b51015: отличия — только
//! переносы строк. В _swapFrom, getSwapOut, _getReserves, _calcReserve, _updatePumps изменений по смыслу нет.
//! Константы ниже — константы контрактов (не настройки).

use alloy_primitives::{Address, U256};

use crate::core::types::{BasinFunction, BasinWellData};

/// ConstantProduct2.sol L24 `EXP_PRECISION = 1e12`.
const EXP_PRECISION: u64 = 1_000_000_000_000;
/// Stable2.sol L34 `N = 2`, L37 `A_PRECISION = 100`.
const N: u64 = 2;
const A_PRECISION: u64 = 100;
/// Stable2.sol: предел итераций `i < 255` (L89, L146).
const ROUNDS: u32 = 255;
/// Знаков, к которым Stable2 приводит резервы (getScaledReserves L419-L426).
const SCALE_DEC: u8 = 18;

/// LibMath.roundUpDiv (L14-L17): a = 0 → 0 (без деления), иначе (a − 1)/b + 1. None — b = 0.
fn round_up_div(a: U256, b: U256) -> Option<U256> {
    if a.is_zero() {
        return Some(U256::ZERO);
    }
    if b.is_zero() {
        return None;
    }
    Some((a - U256::from(1u8)) / b + U256::from(1u8))
}

/// 10^(18 − dec). None — dec > 18 (decodeWellData откатывает InvalidTokenDecimals, L369).
fn scale(dec: u8) -> Option<U256> {
    let e = SCALE_DEC.checked_sub(dec)?;
    Some(U256::from(10u8).pow(U256::from(e)))
}

/// ConstantProduct2.calcReserve (L62-L74): b_j = ceil(s² / (b_{1−j}·1e12)). Принимает: резерв другого токена, s.
/// Возвращает: резерв j или None (переполнение, деление на ноль).
pub fn cp2_reserve(other: U256, supply: U256) -> Option<U256> {
    let s2 = supply.checked_mul(supply)?;
    round_up_div(s2, other.checked_mul(U256::from(EXP_PRECISION))?)
}

/// ConstantProduct2.calcLpTokenSupply (L53-L58): floor(sqrt(b0·b1·1e12)) (OpenZeppelin Math.sqrt — вниз).
/// None — переполнение.
pub fn cp2_supply(r0: U256, r1: U256) -> Option<U256> {
    Some(r0.checked_mul(r1)?.checked_mul(U256::from(EXP_PRECISION))?.root(2))
}

/// Stable2.getBandC (L437-L444): c = s·s/(x·N)·s·A_PRECISION/(Ann·N), b = x + s·A_PRECISION/Ann. None — переполнение
/// или деление на ноль (откат контракта).
fn band_c(ann: U256, s: U256, x: U256) -> Option<(U256, U256)> {
    let n = U256::from(N);
    let ap = U256::from(A_PRECISION);
    let c = s
        .checked_mul(s)?
        .checked_div(x.checked_mul(n)?)?
        .checked_mul(s)?
        .checked_mul(ap)?
        .checked_div(ann.checked_mul(n)?)?;
    let b = x.checked_add(s.checked_mul(ap)?.checked_div(ann)?)?;
    Some((c, b))
}

/// Stable2.calcReserve (L132-L162): итерация x = (x² + c)/(2x + b − s) от x = s до |Δ| ≤ 1, ответ — в знаках токена j
/// (деление вниз на 10^(18 − dec_j)). Принимает: резервы скважины, j, s (totalSupply), A, знаки токенов.
/// Возвращает: резерв j или None (переполнение, вычитание ниже нуля, нет схождения за 255 шагов — откат контракта).
pub fn stable2_reserve(r: [U256; 2], j: usize, s: U256, a: U256, dec: [u8; 2]) -> Option<U256> {
    let x0 = r[0].checked_mul(scale(dec[0])?)?;
    let x1 = r[1].checked_mul(scale(dec[1])?)?;
    let ann = a.checked_mul(U256::from(N * N))?;
    let (c, b) = band_c(ann, s, if j == 0 { x1 } else { x0 })?;
    let mut reserve = s;
    for _ in 0..ROUNDS {
        let prev = reserve;
        // _calcReserve L428-L435: (reserve·reserve + c) / (reserve·2 + b − lpTokenSupply).
        let num = reserve.checked_mul(reserve)?.checked_add(c)?;
        let den = reserve.checked_mul(U256::from(2u8))?.checked_add(b)?.checked_sub(s)?;
        reserve = num.checked_div(den)?;
        let diff = if reserve > prev {
            reserve - prev
        } else {
            prev - reserve
        };
        if diff <= U256::from(1u8) {
            return Some(reserve / scale(dec[j])?);
        }
    }
    None
}

/// Stable2.calcLpTokenSupply (L76-L121), построчно: оба резерва 0 → 0; D от суммы; итерация до |Δ| ≤ 1. Принимает:
/// резервы, A, знаки токенов. Возвращает: D или None (переполнение, деление на ноль, нет схождения — откат).
pub fn stable2_supply(r: [U256; 2], a: U256, dec: [u8; 2]) -> Option<U256> {
    if r[0].is_zero() && r[1].is_zero() {
        return Some(U256::ZERO);
    }
    let x0 = r[0].checked_mul(scale(dec[0])?)?;
    let x1 = r[1].checked_mul(scale(dec[1])?)?;
    let (n, ap) = (U256::from(N), U256::from(A_PRECISION));
    let ann = a.checked_mul(U256::from(N * N))?;
    let sum = x0.checked_add(x1)?;
    let mut d = sum;
    for _ in 0..ROUNDS {
        // L91-L94: dP = D; dP = dP·D/(x0·N); dP = dP·D/(x1·N).
        let dp = d.checked_mul(d)?.checked_div(x0.checked_mul(n)?)?;
        let dp = dp.checked_mul(d)?.checked_div(x1.checked_mul(n)?)?;
        let prev = d;
        let num = ann
            .checked_mul(sum)?
            .checked_div(ap)?
            .checked_add(dp.checked_mul(n)?)?
            .checked_mul(d)?;
        let den = ann
            .checked_sub(ap)?
            .checked_mul(d)?
            .checked_div(ap)?
            .checked_add(n.checked_add(U256::from(1u8))?.checked_mul(dp)?)?;
        d = num.checked_div(den)?;
        // L99-L118: ветка «разница 2 и stableOscillation» не срабатывает никогда — флаг объявлен в теле цикла (L90) и
        // перед проверкой всегда false; остаётся только выход по |Δ| ≤ 1.
        let diff = if d > prev { d - prev } else { prev - d };
        if diff <= U256::from(1u8) {
            return Some(d);
        }
    }
    None
}

/// calcReserve функции скважины. Принимает: состояние (функция), резервы, j, s. Возвращает: резерв j или None.
pub fn calc_reserve(d: &BasinWellData, r: [U256; 2], j: usize, s: U256) -> Option<U256> {
    match d.function {
        BasinFunction::ConstantProduct2 => cp2_reserve(r[1 - j], s),
        BasinFunction::Stable2 { a, dec0, dec1 } => stable2_reserve(r, j, s, a, [dec0, dec1]),
    }
}

/// calcLpTokenSupply функции скважины на резервах состояния (в обмене не участвует; для сверки и тестов).
/// Возвращает: число LP-токенов или None (откат контракта).
pub fn lp_supply(d: &BasinWellData) -> Option<U256> {
    let r = [d.reserve0, d.reserve1];
    match d.function {
        BasinFunction::ConstantProduct2 => cp2_supply(r[0], r[1]),
        BasinFunction::Stable2 { a, dec0, dec1 } => stable2_supply(r, a, [dec0, dec1]),
    }
}

/// Выход обмена swapFrom (Well.sol L243-L260): reserves[i] += вход; выход = reserves[j] − calcReserve(…, j,
/// totalSupply). Сверх getSwapOut — проверка записи резервов `_setReserves` → LibBytes.storeUint128 (L26-L27: резерв
/// ≤ uint128.max, иначе откат). Принимает: состояние, токен входа (монета скважины), вход. Возвращает: выход или None
/// (токен не из скважины, переполнение, вычитание ниже нуля, нет схождения — обмен откатился бы).
pub fn swap_out(d: &BasinWellData, token_in: Address, amount: U256) -> Option<U256> {
    if !d.has(token_in) {
        return None;
    }
    let i = usize::from(token_in != d.token0);
    let j = 1 - i;
    let mut r = [d.reserve0, d.reserve1];
    r[i] = r[i].checked_add(amount)?;
    if r[i] > U256::from(u128::MAX) {
        return None;
    }
    let new_j = calc_reserve(d, r, j, d.supply)?;
    r[j].checked_sub(new_j)
}
