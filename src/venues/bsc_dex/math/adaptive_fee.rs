//! Файл: venues/bsc_dex/math/adaptive_fee.rs
//! Что делает: решение 463 (T463-02) — формула динамической комиссии пула THENA Fusion (линия AlgebraV1): порт
//! AdaptiveFee.getFee / sigmoid / exp (комиссия = baseFee + sigmoidVolume(sigmoid1(волатильность) +
//! sigmoid2(волатильность)), всё в целых, как контракт). Вход — средние оракула (math/fee_algebra_v1.rs).
//! Кто вызывает: math/fee_algebra_v1.rs.
//! От чего зависит: alloy-primitives (U256).
//! Настройки: нет (параметры — feeConfig() оператора пула, читает pools/fusion_fee.rs).
//!
//! Источник: исходник пула THENA Fusion (Sourcify 56, пул 0x1b9a1120…ef7d, полное совпадение) —
//! contracts/libraries/AdaptiveFee.sol (= cryptoalgebra/AlgebraV1 master src/core/contracts/libraries/AdaptiveFee.sol);
//! делитель волатильности 15 — DataStorageOperator.getFee (`volatilityAverage / 15`). Факт записи своего узла
//! (tests/data/bsc_thena_fusion_2026-11-20_t237): getFee оператора = эта формула от getAverages у 60 пулов × 3 времени.

use alloy_primitives::U256;

/// AdaptiveFee.Configuration (feeConfig() оператора, порядок полей ABI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeConfig {
    pub alpha1: u16,
    pub alpha2: u16,
    pub beta1: u32,
    pub beta2: u32,
    pub gamma1: u16,
    pub gamma2: u16,
    pub volume_beta: u32,
    pub volume_gamma: u16,
    pub base_fee: u16,
}

/// AdaptiveFee.getFee: baseFee + sigmoid(volumePerLiquidity, volumeGamma, min(s1 + s2, uint16 max), volumeBeta), где
/// s1, s2 — сигмоиды волатильности. Итог — uint16 (контракт обрезает; alpha1 + alpha2 + baseFee ≤ uint16 max по
/// changeFeeConfiguration). Принимает: волатильность (уже / 15), объём на ликвидность, параметры. Возвращает:
/// комиссию (пипсы, 1e-6); None — gamma = 0 (в контракте — деление на ноль, откат). Ошибок нет.
pub fn get_fee(volatility: U256, volume_per_liquidity: U256, c: &FeeConfig) -> Option<u32> {
    let s = sigmoid(volatility, c.gamma1, c.alpha1, U256::from(c.beta1))?
        + sigmoid(volatility, c.gamma2, c.alpha2, U256::from(c.beta2))?;
    let s = s.min(U256::from(u16::MAX)).to::<u16>();
    let beta = U256::from(c.volume_beta);
    let v = sigmoid(volume_per_liquidity, c.volume_gamma, s, beta)?;
    // uint16(baseFee + v): контракт берёт младшие 16 бит.
    Some(((U256::from(c.base_fee) + v).to::<u128>() & 0xffff) as u32)
}

/// sigmoid: α / (1 + e^((β − x) / γ)) в целых (AdaptiveFee.sol). Принимает: x, γ, α, β. Возвращает: значение (≤ α);
/// None — γ = 0. Ошибок нет.
fn sigmoid(x: U256, g: u16, alpha: u16, beta: U256) -> Option<U256> {
    if g == 0 {
        return None;
    }
    let g6 = U256::from(6u8) * U256::from(g);
    let a = U256::from(alpha);
    let g8 = U256::from(g).pow(U256::from(8u8));
    if x > beta {
        let x = x - beta;
        if x >= g6 {
            return Some(a);
        }
        let ex = exp(x, g, g8);
        Some(a * ex / (g8 + ex))
    } else {
        let x = beta - x;
        if x >= g6 {
            return Some(U256::ZERO);
        }
        let ex = g8 + exp(x, g, g8);
        Some(a * g8 / ex)
    }
}

/// exp: e^(x/g) · g^8 рядом до x^8 (AdaptiveFee.sol: g^8 + x·g^7 + x²·g^6/2 + … + x^8/40320; деления — по шагам, как в
/// контракте). Принимает: x (< 6g), g (> 0), g^8. Возвращает: значение. Ошибок нет.
fn exp(x: U256, g: u16, g_highest: U256) -> U256 {
    let g = U256::from(g);
    let mut gh = g_highest;
    let mut xl = x;
    let mut res = gh;
    gh /= g;
    res += xl * gh;
    for div in [2u32, 6, 24, 120, 720] {
        gh /= g;
        xl *= x;
        res += xl * gh / U256::from(div);
    }
    xl *= x;
    res + xl * g / U256::from(5040u32) + xl * x / U256::from(40320u32)
}
