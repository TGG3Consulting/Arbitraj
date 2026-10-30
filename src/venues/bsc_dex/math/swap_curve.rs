//! Файл: venues/bsc_dex/math/swap_curve.rs
//! Что делает: уровень свопа Curve StableSwap (решение 80 — экзотическая математика Curve/Ellipsis stableswap) —
//! порт `get_dy` официального контракта: сколько монеты j получит трейдер за `dx` монеты i, с комиссией пула.
//! Инвариант (get_d/get_y) — в math/curve_math.rs. Та же математика покрывает Ellipsis на BSC (форк Curve,
//! инвариант тот же). Это уровень цены ОДНОГО свопа в одном плоском пуле; граф/маршрут/сплит (решение 80), чтение
//! состояния пула, события, квотер и исполнение — следующая задача (семейство Curve/Ellipsis, фаза B).
//! Охват фазы A: PLAIN StableSwap с РАВНЫМИ rate-множителями (RATES[k] = PRECISION, все монеты 18 знаков) → xp =
//! balances, а умножения `· rates[i]/PRECISION` на входе и `· PRECISION/rates[j]` на выходе — тождественны. Разные
//! decimals/rates и meta-пулы — фаза B. `amp` — A С ПРЕФАКТОРОМ A_PRECISION (как `self._A()` контракта = A·100).
//! Кто вызывает: quote/pool_quote.rs (через семейство Curve, фаза B); тесты (tests/bsc_dex_curve_math.rs).
//! От чего зависит: math/curve_math.rs. Настройки: нет (balances, A, fee — с контракта пула в фазе B).
//!
//! Источники (официальные, решение 23): curvefi/curve-contract (HEAD 574f44027d089de0eac765f5a74ea5ae96aba968),
//! contracts/pool-templates/base/SwapTemplateBase.vy (@version ^0.2.8): `get_dy`. Vyper 0.2.x — вся арифметика
//! checked → порт на checked_*, где контракт откатился бы (недобор, деление на ноль, переполнение) — `None`.

use alloy_primitives::U256;

// Инвариант и константы (math/curve_math.rs) — для котировки (фаза B) и тестов — доступны через этот модуль.
pub use super::curve_math::{a_precision, fee_denominator, get_d, get_y, precision};

/// SwapTemplateBase.get_dy: выход свопа dx монеты i → монета j с комиссией пула `fee` (доля от FEE_DENOMINATOR=10^10).
/// Охват фазы A — равные rate-множители (RATES[k]=PRECISION): xp = balances, конверсии входа/выхода тождественны.
/// Принимает: i, j — индексы монет; dx — вход монеты i; balances — текущие балансы (N монет, в единицах монеты);
/// amp — A·A_PRECISION; fee — числитель комиссии над 10^10. Возвращает: dy (выход монеты j) за вычетом комиссии.
/// None — неверные индексы, недобор (`xp[j] − y − 1` ниже нуля при вырожденных данных), переполнение или деление на
/// ноль внутри get_y/get_d (см. curve_math), либо несходимость Ньютона.
pub fn get_dy(
    i: usize,
    j: usize,
    dx: U256,
    balances: &[U256],
    amp: U256,
    fee: U256,
) -> Option<U256> {
    let n = balances.len();
    if i == j || i >= n || j >= n {
        return None;
    }
    // x = xp[i] + dx·rates[i]/PRECISION; при равных rate-множителях rates[i]=PRECISION → x = balances[i] + dx.
    let x = balances[i].checked_add(dx)?;
    let y = get_y(i, j, x, balances, amp)?;
    // dy = xp[j] − y − 1 (контракт держит запас −1). Недобор → None.
    let dy = balances[j].checked_sub(y)?.checked_sub(U256::from(1u8))?;
    // fee = self.fee · dy / FEE_DENOMINATOR; выход = (dy − fee)·PRECISION/rates[j] = dy − fee при равных множителях.
    let f = fee.checked_mul(dy)? / fee_denominator();
    dy.checked_sub(f)
}
