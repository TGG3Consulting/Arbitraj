//! Файл: core/types/pool_crypto.rs
//! Что делает: состояние crypto-пула Curve (решение 442: Twocrypto-NG — 2 монеты, Tricrypto-NG — 3 монеты) — без
//! названия площадки: балансы монет, множители точности, внутренняя цена монет к монете 0 (price_scale), инвариант D,
//! параметры A и gamma, параметры комиссии (mid_fee, out_fee, fee_gamma), остановка обменов и индексы пары. Комиссию
//! и выход обмена считает математика адаптера (по числу монет — формула Twocrypto или Tricrypto). Вынесено из pool.rs
//! (предел размера файла).
//! Кто вызывает: core/types/pool.rs (PoolState::Crypto), адаптер DEX (pools/load_crypto.rs создаёт, math/crypto_swap.rs
//! считает).
//! От чего зависит: serde, alloy-primitives.
//! Настройки: нет.

use alloy_primitives::U256;
use serde::{Deserialize, Serialize};

/// Состояние crypto-пула Curve на блоке — боксируется в `PoolState::Crypto`. Числа — как отдаёт пул (balances(k),
/// precisions(), price_scale(), D(), A(), gamma(), mid_fee(), out_fee(), fee_gamma()).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CryptoData {
    /// Балансы монет пула по порядку монет пула (`balances(k)`), в единицах монеты.
    pub balances: Vec<U256>,
    /// Множители точности монет (`precisions()`: 10^(18 − decimals)).
    pub precisions: Vec<U256>,
    /// Цена монет 1..N−1 к монете 0 в 18 знаках (`price_scale(k)` / `price_scale()`), N − 1 значений.
    pub price_scale: Vec<U256>,
    /// Инвариант D (`D()`).
    pub d: U256,
    /// A·N^N·10000 (`A()` — как его получает математика пула).
    pub a: U256,
    /// gamma (`gamma()`), 18 знаков.
    pub gamma: U256,
    /// Комиссия в равновесии — числитель над 10^10 (`mid_fee()`).
    pub mid_fee: U256,
    /// Комиссия вдали от равновесия — числитель над 10^10 (`out_fee()`).
    pub out_fee: U256,
    /// Скорость перехода комиссии от mid к out (`fee_gamma()`), 18 знаков.
    pub fee_gamma: U256,
    /// Обмены считаются неисполнимыми: идёт рампа A/gamma (обмен пересчитал бы D — без этого расчёт не точен) или
    /// чтение не дало всех данных.
    pub halted: bool,
    /// Индекс монеты token0 в пуле.
    pub i0: u8,
    /// Индекс монеты token1 в пуле.
    pub i1: u8,
}
