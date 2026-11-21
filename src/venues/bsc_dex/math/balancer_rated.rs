//! Файл: venues/bsc_dex/math/balancer_rated.rs
//! Что делает: решение 473 — обмен стейблсвапа Balancer V2 с курсами (composable stable, meta-stable, stable phantom)
//! при данном наборе множителей FixedPoint: комиссия со входа, множители, BPT пула выкинут из балансов, StableMath.
//! Вынесено из math/balancer_v2_more.rs (предел размера файла; там же — выбор худшего из двух наборов множителей).
//! Кто вызывает: math/balancer_v2_more.rs.
//! От чего зависит: core/types (VaultPoolData), math/balancer_math.rs (FixedPoint), math/balancer_stable.rs.
//! Настройки: нет.
//!
//! Источник — шапка math/balancer_v2_more.rs (BaseGeneralPool.sol L32-L85, ComposableStablePool.sol L279-L298,
//! MetaStablePool.sol L125-L145, StablePhantomPool.sol L207-L271).

use alloy_primitives::U256;

use crate::core::types::VaultPoolData;

use super::balancer_math::{div_down, div_up, mul_down, mul_up, one};
use super::balancer_stable;

/// complement: 1 − x, не меньше нуля (FixedPoint.complement).
fn complement(x: U256) -> U256 {
    if x < one() { one() - x } else { U256::ZERO }
}

/// Параметры стейблсвапа с курсами: A × точность, индекс BPT, StableMath v2.
pub type Rated = (U256, Option<u8>, bool);

/// Выход стейблсвапа с курсами при данных множителях (BaseGeneralPool._swapGivenIn). None — откат.
pub fn out(d: &VaultPoolData, r: Rated, f: &[U256], i: usize, j: usize, a: U256) -> Option<U256> {
    let (amp, bpt, v2) = r;
    let (f_in, f_out) = (*f.get(i)?, *f.get(j)?);
    let (b, ii, jj) = rated_balances(d, f, bpt, i, j)?;
    let up = mul_down(a.checked_sub(mul_up(a, d.fee)?)?, f_in)?;
    let out = balancer_stable::out_given_in(amp, &b, ii, jj, up, v2)?;
    div_down(out, f_out)
}

/// Вход стейблсвапа с курсами при данных множителях (BaseGeneralPool._swapGivenOut). None — откат.
pub fn in_(d: &VaultPoolData, r: Rated, f: &[U256], i: usize, j: usize, a: U256) -> Option<U256> {
    let (amp, bpt, v2) = r;
    let (f_in, f_out) = (*f.get(i)?, *f.get(j)?);
    let (b, ii, jj) = rated_balances(d, f, bpt, i, j)?;
    let up = mul_down(a, f_out)?;
    let raw = balancer_stable::in_given_out(amp, &b, ii, jj, up, v2)?;
    div_up(div_up(raw, f_in)?, complement(d.fee))
}

/// Балансы стейблсвапа × множители FixedPoint (mulDown) без BPT пула и индексы пары среди них. None — пара с BPT,
/// длины не совпали, переполнение.
fn rated_balances(
    d: &VaultPoolData,
    factors: &[U256],
    bpt: Option<u8>,
    i: usize,
    j: usize,
) -> Option<(Vec<U256>, usize, usize)> {
    let skip = bpt.map(usize::from);
    if skip == Some(i) || skip == Some(j) || d.balances.len() != factors.len() {
        return None;
    }
    let mut out = Vec::with_capacity(d.balances.len());
    for (k, (b, f)) in d.balances.iter().zip(factors).enumerate() {
        if Some(k) != skip {
            out.push(mul_down(*b, *f)?);
        }
    }
    let shift = |x: usize| x - usize::from(skip.is_some_and(|s| x > s));
    Some((out, shift(i), shift(j)))
}
