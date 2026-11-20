//! Файл: core/types/pool_smart.rs
//! Что делает: состояние пула «с центральной ценой и мнимыми резервами на заёмной ликвидности» (решение 440) — без
//! названия площадки: пул держит ликвидность не у себя, а в общем слое ликвидности как «смарт-залог» (обмен = внести
//! вход и снять выход) и/или «смарт-долг» (обмен = погасить долг входом и занять выход); у каждой части — реальные и
//! мнимые резервы (по мнимым — формула x·y=k), обмен делится между частями так, чтобы их цены после обмена совпали.
//! Всё — в «приведённых» единицах пула (10^12 на токен; множители точности — поля), кроме пределов снятия/займа (в
//! единицах токена). Вынесено из pool.rs (предел размера файла).
//! Кто вызывает: core/types/pool.rs (PoolState::SmartColDebt), адаптер DEX (pools/load_fluid.rs создаёт,
//! math/fluid_swap.rs считает).
//! От чего зависит: alloy-primitives, serde.
//! Настройки: нет.

use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};

/// Резервы части «смарт-залог» (приведённые единицы): реальные и мнимые по token0/token1 пула.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ColReserves {
    pub real0: U256,
    pub real1: U256,
    pub imag0: U256,
    pub imag1: U256,
}

/// Резервы части «смарт-долг» (приведённые единицы): долг, реальные и мнимые резервы по token0/token1 пула.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DebtReserves {
    pub debt0: U256,
    pub debt1: U256,
    pub real0: U256,
    pub real1: U256,
    pub imag0: U256,
    pub imag1: U256,
}

/// Состояние пула на блоке — боксируется в `PoolState::SmartColDebt`. Математика — в адаптере.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmartColDebtData {
    /// token0 САМОГО пула (порядок пула, не по адресу; нативная монета уже заменена обёрткой) — по нему направление.
    pub token0: Address,
    /// token0 / token1 пула — нативная монета (в паре графа стоит обёртка).
    pub native0: bool,
    pub native1: bool,
    /// Включены части «смарт-залог» и «смарт-долг».
    pub col_on: bool,
    pub debt_on: bool,
    /// Обмен на паузе.
    pub paused: bool,
    /// Комиссия обмена (1e6 = 100 %).
    pub fee: u32,
    /// Доля протокола в комиссии (проценты, 0–100).
    pub revenue_cut: u32,
    /// Центральная цена пула (1e27).
    pub center_price: U256,
    pub col: ColReserves,
    pub debt: DebtReserves,
    /// Сколько можно снять (залог) и занять (долг) по token0/token1 — единицы токена.
    pub withdrawable0: U256,
    pub withdrawable1: U256,
    pub borrowable0: U256,
    pub borrowable1: U256,
    /// Множители точности: приведённое = сумма · num / den (у token0 и token1 свои).
    pub num0: U256,
    pub den0: U256,
    pub num1: U256,
    pub den1: U256,
    /// Слово состояния пула со сохранёнными ценами и временем последнего обмена (для проверки скачка цены).
    pub dex_variables: U256,
    /// Пул новой версии (есть VERSION()) — проверка скачка цены по сохранённой цене; иначе — по текущей цене пула.
    pub versioned: bool,
    /// Слой ликвидности пула — от него приходит нативная монета при выходе в неё.
    pub liquidity: Address,
    /// Время блока чтения (секунды Unix) — к нему проверка скачка цены.
    pub block_ts: u64,
}
