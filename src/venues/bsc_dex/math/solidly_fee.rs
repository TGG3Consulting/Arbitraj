//! Файл: venues/bsc_dex/math/solidly_fee.rs
//! Что делает: правило комиссии пула Solidly (решение 273) — вход после комиссии, комиссия в процентах, доля «после
//! комиссии» для обратной V2-формулы — по правилу из состояния пула (core/types/pool_solidly.rs), и проверка
//! ограничений обмена пары (пауза фабрики, предел цены газа). Правила:
//! - числитель: `amountIn -= amountIn * fee / denominator` — Velodrome Pool.sol `getAmountOut` (HEAD b3065d8),
//!   THENA V1 Pair.sol L461 (HEAD f5bc742), делитель 10000;
//! - делитель: `amountIn -= amountIn / fee` — usdfi BasePair (verified, solc 0.8.17,
//!   https://bscscan.com/address/0xb3863573d9f25e6a84895d4685a408db7a488416#code — фабрика, пары по её коду).
//!
//! Solidity 0.8 — checked: деление на ноль и недобор откатываются → None.
//! Кто вызывает: quote/pool_quote.rs, quote/steps.rs, quote/emptiness.rs, exec/solidly_gate.rs; тесты.
//! От чего зависит: core/types (SolidlyFee, FeeRule, SolidlyGate), rust_decimal.
//! Настройки: нет (правило — в состоянии пула, его задаёт pools/load_solidly.rs по dex.solidly_fee_* [DEX]).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{FeeRule, SolidlyFee, SolidlyGate};

/// Вход после комиссии пары. Принимает: вход, комиссию с правилом. Возвращает: вход за вычетом комиссии так же, как
/// контракт (целое деление). None — делитель 0 (откат контракта), переполнение, недобор.
pub fn net_in(amount: U256, fee: &SolidlyFee) -> Option<U256> {
    let v = U256::from(fee.value);
    let cut = match fee.rule {
        FeeRule::Numerator { denominator } => {
            if denominator == 0 {
                return None;
            }
            amount.checked_mul(v)? / U256::from(denominator)
        }
        FeeRule::Divisor => {
            if v.is_zero() {
                return None;
            }
            amount / v
        }
    };
    amount.checked_sub(cut)
}

/// Комиссия в процентах. Числитель: value / denominator · 100 (делитель 10000 → value / 100, как до решения 273);
/// делитель: 100 / value. None — делитель 0.
pub fn pct(fee: &SolidlyFee) -> Option<Decimal> {
    let v = Decimal::from(fee.value);
    match fee.rule {
        FeeRule::Numerator { denominator: 0 } => None,
        FeeRule::Divisor if fee.value == 0 => None,
        FeeRule::Numerator { denominator } if denominator % 100 == 0 => {
            Some(v / Decimal::from(denominator / 100))
        }
        FeeRule::Numerator { denominator } => {
            Some(v * Decimal::ONE_HUNDRED / Decimal::from(denominator))
        }
        FeeRule::Divisor => Some(Decimal::ONE_HUNDRED / v),
    }
}

/// Точная причина, почему pct = None (ревью 273): числитель — делитель правила 0; делитель — значение комиссии 0
/// (контракт делил бы на ноль). Ошибок нет.
pub fn no_pct(fee: &SolidlyFee) -> String {
    match fee.rule {
        FeeRule::Numerator { denominator } => format!(
            "комиссия Solidly {}: правило «числитель над делителем», делитель {denominator} — процент не считается",
            fee.value
        ),
        FeeRule::Divisor => format!(
            "комиссия Solidly {}: правило «делитель» (вход − вход / комиссия), значение 0 — деление на ноль в контракте",
            fee.value
        ),
    }
}

/// Доля входа после комиссии дробью (числитель, знаменатель) — для обратной V2-формулы точного выхода (getAmountIn у
/// контракта нет). Числитель: (denominator − value, denominator); делитель: (value − 1, value). None — комиссия ≥ 100%.
pub fn keep_ratio(fee: &SolidlyFee) -> Option<(U256, U256)> {
    let (keep, of) = match fee.rule {
        FeeRule::Numerator { denominator } => (denominator.checked_sub(fee.value)?, denominator),
        FeeRule::Divisor => (fee.value.checked_sub(1)?, fee.value),
    };
    (keep > 0).then(|| (U256::from(keep), U256::from(of)))
}

/// Почему обмен пары откатится при этой цене газа (решение 273). Принимает: ограничения пары, цену газа транзакции
/// (None — цена ещё не известна, проверяется только пауза). Возвращает: точную причину или None — обмен возможен.
pub fn blocked(gate: &SolidlyGate, gas_price: Option<U256>) -> Option<String> {
    if gate.paused {
        return Some("фабрика пары на паузе (isPaused) — обмен откатился бы".into());
    }
    match (gate.max_gas_price, gas_price) {
        (Some(max), Some(p)) if p > U256::from(max) => Some(format!(
            "цена газа сделки {p} wei выше предела пары maxGasPrice {max} wei — обмен откатился бы"
        )),
        _ => None,
    }
}
