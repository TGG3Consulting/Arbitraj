//! Файл: venues/bsc_dex/math/balancer_v3_hook.rs
//! Что делает: решение 452 — комиссия обмена пула Balancer V3 так, как её берёт хранилище: статическая комиссия пула
//! или ответ крючка onComputeDynamicSwapFeePercentage (Vault.sol 20241204-v3-vault L246-L253; ответ > 99,9999 % —
//! откат, HooksConfigLib.sol L191-L195). Крючки — официальные Balancer с развёртыванием на Arbitrum: StableSurge (обе
//! версии — одна формула) и MEV Capture. Крючок получает полную сумму обмена до комиссии (PoolSwapParams до _swap).
//! Кто вызывает: math/balancer_v3.rs (amount_out, amount_in).
//! От чего зависит: core/types (VaultPoolData, VaultV3, V3Hook), math/balancer_v3_pool.rs (onSwap пула — его зовёт
//! StableSurge), math/balancer_math.rs (mulDown, divDown), math/balancer_v3_weighted.rs (complement).
//! Настройки: нет (параметры крючка — pools/load_balancer_v3*.rs).
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 build-info:
//! - v3/tasks/20250403-v3-stable-surge-hook-v2 → contracts/StableSurgeHook.sol L123-L150 (onCompute… → getSurgeFeePercentage:
//!   onSwap пула с полной суммой, новые балансы: вход + сумма, выход − расчёт; точный выход — наоборот), L237-L274
//!   (_getSurgeFeePercentage: наибольшая < статической → статическая; иначе при «всплеске» статическая + (наибольшая −
//!   статическая)·mulDown((новый дисбаланс − порог) divDown complement(порог))), L276-L290 (_isSurging: новый дисбаланс 0
//!   — нет; иначе новый > старого и > порога); contracts/utils/StableSurgeMedianMath.sol L12-L41 (дисбаланс = Σ|b − медиана|
//!   divDown Σb; медиана — средняя пары при чётном числе, целое деление). v3/deprecated/20250121-v3-stable-surge — те же
//!   функции (diff: только проверка фабрики при регистрации и права настройки);
//! - v3/tasks/20250212-v3-mev-capture-hook → contracts/MevCaptureHook.sol L90-L113 (выключен — статическая; освобождение
//!   отправителя — только у доверенного роутера, исполнитель им не является), L311-L345 (_calculateSwapFeePercentage:
//!   чаевые ≤ порога или наибольшая ≤ статической — статическая; переполнение (чаевые − порог)·множитель — наибольшая;
//!   иначе min(статическая + произведение / 1e18, наибольшая)), L353-L355 (чаевые = tx.gasprice − block.basefee).

use alloy_primitives::U256;

use crate::core::types::pool_vault::{V3Hook, VaultPoolData, VaultV3};

use super::balancer_math::{div_down, mul_down, one};
use super::balancer_v3_pool::on_swap;
use super::balancer_v3_weighted::complement;

/// MAX_FEE_PERCENTAGE = 99.9999e16 (VaultTypes.sol L439) — ответ крючка больше — откат хранилища.
fn max_fee() -> U256 {
    U256::from(999_999u32) * super::ten::pow(12)
}

/// Комиссия обмена. Принимает: состояние, часть V3, индексы, полную сумму (18 знаков), точный ли вход. Возвращает:
/// комиссию (18 знаков). None — откат крючка (onSwap пула, деление на ноль) или ответ > 99,9999 %.
pub fn fee(
    d: &VaultPoolData,
    v3: &VaultV3,
    i: usize,
    j: usize,
    given: U256,
    exact_in: bool,
) -> Option<U256> {
    let st = d.fee;
    let f = match &v3.hook {
        V3Hook::None => return Some(st),
        V3Hook::Surge { max, threshold } => {
            surge(d, v3, i, j, given, exact_in, st, *max, *threshold)?
        }
        V3Hook::Mev {
            enabled,
            max,
            multiplier,
            threshold,
            priority,
        } => mev(*enabled, st, *max, *multiplier, *threshold, *priority)?,
    };
    (f <= max_fee()).then_some(f)
}

/// StableSurge (StableSurgeHook.sol L131-L150, L237-L290). Крючок сам зовёт onSwap пула: у пула с onlyVault вызов
/// откатывается (open = false — None). None — откат.
#[allow(clippy::too_many_arguments)]
fn surge(
    d: &VaultPoolData,
    v3: &VaultV3,
    i: usize,
    j: usize,
    given: U256,
    exact_in: bool,
    st: U256,
    max: U256,
    thr: U256,
) -> Option<U256> {
    if !v3.open {
        return None;
    }
    let calc = on_swap(&d.curve, v3, i, j, given, exact_in)?;
    let mut new = v3.live.clone();
    let (add, sub) = if exact_in {
        (given, calc)
    } else {
        (calc, given)
    };
    *new.get_mut(i)? = new.get(i)?.checked_add(add)?;
    *new.get_mut(j)? = new.get(j)?.checked_sub(sub)?;
    if max < st {
        return Some(st);
    }
    let new_imb = imbalance(&new)?;
    // _isSurging: новый дисбаланс 0 — не всплеск (старый не считается); иначе новый > старого и > порога.
    let surging = if new_imb.is_zero() {
        false
    } else {
        let old = imbalance(&v3.live)?;
        new_imb > old && new_imb > thr
    };
    if !surging {
        return Some(st);
    }
    let part = div_down(new_imb.checked_sub(thr)?, complement(thr))?;
    st.checked_add(mul_down(max.checked_sub(st)?, part)?)
}

/// calculateImbalance (StableSurgeMedianMath.sol L12-L24): Σ|b − медиана| divDown Σb. None — сумма 0 (деление на ноль)
/// или переполнение.
fn imbalance(b: &[U256]) -> Option<U256> {
    let mut sorted = b.to_vec();
    sorted.sort();
    let mid = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        sorted
            .get(mid.checked_sub(1)?)?
            .checked_add(*sorted.get(mid)?)?
            / U256::from(2u8)
    } else {
        *sorted.get(mid)?
    };
    let (mut total, mut diff) = (U256::ZERO, U256::ZERO);
    for x in b {
        total = total.checked_add(*x)?;
        let d = if *x > median {
            *x - median
        } else {
            median - *x
        };
        diff = diff.checked_add(d)?;
    }
    div_down(diff, total)
}

/// MEV Capture (MevCaptureHook.sol L95-L112, L311-L345). Принимает: включён ли налог, статическую, наибольшую,
/// множитель, порог, чаевые транзакции. Возвращает: комиссию. None — переполнение суммы (откат Solidity 0.8).
fn mev(enabled: bool, st: U256, max: U256, mult: U256, thr: U256, prio: U256) -> Option<U256> {
    if !enabled || prio <= thr || max <= st {
        return Some(st);
    }
    let Some(inc) = (prio - thr).checked_mul(mult) else {
        return Some(max);
    };
    Some(st.checked_add(inc / one())?.min(max))
}
