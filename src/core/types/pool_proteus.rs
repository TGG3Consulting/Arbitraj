//! Файл: core/types/pool_proteus.rs
//! Что делает: состояние пула «кусочная гипербола за общим учётным контрактом» (решение 486, Shell Protocol Proteus) —
//! без названия площадки: токены всех пулов лежат в одном учётном контракте (ERC-1155 учёт), пул — отдельный контракт
//! (прокси) с внешней реализацией формулы; кривая — несколько «срезов» (slices) по отношению y/x, в каждом срезе
//! гипербола ((x/(k·u)) + a)((y/(k·u)) + b) = 1 с полезностью u (u при обмене не меняется). Балансы — в единицах учёта
//! (18 знаков у любого токена), параметры срезов — числа 64.64 (ABDK). Комиссия — округлением суммы входа и выхода
//! (BASE_FEE, FIXED_FEE реализации). Вынесено из pool.rs (предел размера файла).
//! Кто вызывает: core/types/pool.rs (PoolState::Proteus), адаптер DEX (pools/load_shell.rs создаёт,
//! math/proteus_*.rs считает).
//! От чего зависит: alloy-primitives, serde.
//! Настройки: нет.

use alloy_primitives::{Address, I256, U256};
use serde::{Deserialize, Serialize};

/// Формула полезности реализации (решение 486): исходники развёрнутых реализаций Proteus различаются только ею и
/// BASE_FEE (сверка Sourcify ↔ Shell-Protocol/Shell-Protocol, docs/DECISIONS.md «Решение 486»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProteusUtility {
    /// Корень через обратный знаменатель: u = (bx + ay ± √((bx)² + 4xy − 2aybx + (ay)²)) / k(2 − 2ab), больший корень
    /// (Shell-Protocol bfbba01 src/proteus/Proteus.sol L547-L568).
    V1,
    /// Квадратное уравнение k(ab − 1)u² + (ay + bx)u + xy/k = 0 в масштабе 1e18; при a < 0 и b < 0 — меньший корень,
    /// иначе больший (реализации Sourcify 42161, например 0x243e8336… Proteus.sol L551-L570).
    V2,
}

/// Состояние пула на блоке — боксируется в `PoolState::Proteus`. Математика — в адаптере.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProteusData {
    /// Учётный контракт пула (ocean() пула) — у него балансы и через него обмен.
    pub ocean: Address,
    /// Реализация формулы (implementation() прокси пула) на блоке чтения.
    pub implementation: Address,
    /// id токенов пула в учёте (xToken() / yToken()).
    pub x_id: U256,
    pub y_id: U256,
    /// Токен пары графа на стороне x / y (у нативной монеты учёта — обёртка dex.native_wrapped).
    pub token_x: Address,
    pub token_y: Address,
    /// Сторона x / y — нативная монета учёта (id «Ether»), а не ERC-20.
    pub native_x: bool,
    pub native_y: bool,
    /// Знаков токена ERC-20 стороны (decimals()); у нативной монеты — 18.
    pub dec_x: u8,
    pub dec_y: u8,
    /// Балансы пула в учёте (balanceOfBatch учётного контракта) — единицы учёта, 18 знаков.
    pub x_bal: U256,
    pub y_bal: U256,
    /// Наклоны границ срезов (getSlopes), сдвиги a, b и относительная ликвидность k срезов (getAs / getBs / getKs) —
    /// числа 64.64 (int128 ABDK), по порядку реализации (срез 0 — у оси x).
    pub slopes: Vec<I256>,
    pub a: Vec<I256>,
    pub b: Vec<I256>,
    pub k: Vec<I256>,
    /// BASE_FEE реализации (делитель округления: сумма / BASE_FEE).
    pub base_fee: U256,
    /// Формула полезности реализации — из списков dex.proteus_utility_v1 / v2 (сверенный исходник).
    pub utility: ProteusUtility,
    /// Делитель комиссии вывода учётного контракта (unwrapFeeDivisor; комиссия = сумма / делитель).
    pub unwrap_fee_divisor: U256,
    /// Пул заморожен владельцем (poolFrozen) — обмены откатываются.
    pub frozen: bool,
}

impl ProteusData {
    /// Токен пары — сторона x пула (иначе y). None — токен не из пула. Ошибок нет.
    pub fn is_x(&self, token: Address) -> Option<bool> {
        if token == self.token_x {
            Some(true)
        } else if token == self.token_y {
            Some(false)
        } else {
            None
        }
    }
}
