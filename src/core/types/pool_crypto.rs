//! Файл: core/types/pool_crypto.rs
//! Что делает: состояние crypto-пула Curve (решение 442: Twocrypto-NG — 2 монеты, Tricrypto-NG — 3 монеты; решение
//! 453: Twocrypto-NG v2.0.0, старые crypto-пулы v1 — tricrypto 2021 и фабрика crypto v1) — без названия площадки:
//! балансы монет, множители точности, внутренняя цена монет к монете 0 (price_scale), инвариант D, параметры A и gamma,
//! параметры комиссии (mid_fee, out_fee, fee_gamma), какой исходник математики считает пул (CryptoMath), рампа A/gamma,
//! остановка обменов, приём нативной монеты без события (решение 455) и индексы пары. Комиссию и выход обмена считает
//! математика адаптера. Вынесено из pool.rs (предел размера файла).
//! Кто вызывает: core/types/pool.rs (PoolState::Crypto), адаптер DEX (pools/load_crypto.rs создаёт, math/crypto_swap.rs
//! считает).
//! От чего зависит: serde, alloy-primitives.
//! Настройки: нет.

use alloy_primitives::U256;
use serde::{Deserialize, Serialize};

/// Исходник математики пула (решение 453): формулы get_y / newton_D у версий разные — пул считается формулой своей
/// версии. Какая версия у пула, решает адаптер по фактам (MATH() пула и список адресов математики в настройках DEX,
/// вид DEX).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CryptoMath {
    /// Twocrypto-NG v2.1.0 (curvefi/twocrypto-ng 89c8dfb).
    TwoNg210,
    /// Twocrypto-NG v2.0.0 (curvefi/twocrypto-ng d612fde).
    TwoNg200,
    /// Tricrypto-NG (curvefi/tricrypto-ng ecaa816).
    TriNg,
    /// Фабрика crypto v1, CurveCryptoSwap2ETH.vy (curvefi/curve-factory-crypto 7ed1011), математика внутри пула.
    TwoV1,
    /// Tricrypto 2021, CryptoSwap.vy + CurveCryptoMath3.vy Vyper 0.2.15 (curvefi/curve-crypto-contract d7d04cd).
    TriV1,
}

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
    /// Исходник математики пула (решение 453).
    pub math: CryptoMath,
    /// Обмен сначала пересчитывает D через newton_D по балансам до обмена (рампа A/gamma: у NG — future_A_gamma_time
    /// больше времени блока, у v1 — future_A_gamma_time > 0).
    pub ramp: bool,
    /// A и gamma меняются со временем блока (время блока чтения < future_A_gamma_time): пул перечитывается каждым
    /// блоком, пока это так (решение 453).
    pub moving: bool,
    /// Обмены пула остановлены (tricrypto v1: is_killed()).
    pub halted: bool,
    /// Решение 455: пул принимает нативную монету сети без события и берёт её баланс в состояние (какие пулы — решает
    /// адаптер по исходнику вида и монетам пула) — перечитывается каждым блоком.
    #[serde(default)]
    pub native_in: bool,
    /// Индекс монеты token0 в пуле.
    pub i0: u8,
    /// Индекс монеты token1 в пуле.
    pub i1: u8,
}
