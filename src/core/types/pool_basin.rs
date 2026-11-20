//! Файл: core/types/pool_basin.rs
//! Что делает: состояние «скважины» (Well) Basin (решение 485) — без названия площадки: пул из двух токенов, у
//! которого нет своей комиссии, а цену задаёт внешняя «функция скважины» (well function): резервы токенов в порядке
//! самого пула, число LP-токенов (totalSupply) — оно входит в формулу обмена, и вид функции с её параметрами
//! (x·y = s² — ConstantProduct2; стейблсвап — Stable2 с A и десятичными знаками токенов). Вынесено из pool.rs (предел
//! размера файла).
//! Кто вызывает: core/types/pool.rs (PoolState::BasinWell), адаптер DEX (pools/load_basin.rs создаёт,
//! math/basin_well.rs считает).
//! От чего зависит: alloy-primitives, serde.
//! Настройки: нет.

use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};

/// Функция скважины — какой формулой пул считает обмен (BeanstalkFarms/Basin src/functions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BasinFunction {
    /// ConstantProduct2: b0·b1·1e12 = s² (ConstantProduct2.sol L53-L74).
    ConstantProduct2,
    /// Stable2: стейблсвап из двух токенов (Stable2.sol L76-L162). `a` — параметр A с точностью 100
    /// (getAParameter() таблицы LUT, из которой его берёт конструктор Stable2, L60-L66); `dec0` / `dec1` — десятичные
    /// знаки токенов из данных функции скважины (decodeWellData L357-L374: 0 → 18, больше 18 — откат).
    Stable2 { a: U256, dec0: u8, dec1: u8 },
}

/// Состояние скважины на блоке — боксируется в `PoolState::BasinWell`. Математика — в адаптере.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasinWellData {
    /// token0 САМОЙ скважины (tokens()[0], порядок пула, не по адресу) — по нему индексы i / j обмена.
    pub token0: Address,
    /// tokens()[1] скважины.
    pub token1: Address,
    /// Резервы по порядку токенов скважины (getReserves(): uint128 каждый, LibBytes.readUint128).
    pub reserve0: U256,
    pub reserve1: U256,
    /// Число LP-токенов скважины (totalSupply()) — аргумент calcReserve при обмене (Well.sol L249, L278).
    pub supply: U256,
    /// Функция скважины.
    pub function: BasinFunction,
}

impl BasinWellData {
    /// Токен — одна из монет скважины. Ошибок нет.
    pub fn has(&self, t: Address) -> bool {
        t == self.token0 || t == self.token1
    }

    /// Резервы (вход, выход) по токену входа: вход — token0 скважины → (reserve0, reserve1). Ошибок нет.
    pub fn sides(&self, token_in: Address) -> (U256, U256) {
        if token_in == self.token0 {
            (self.reserve0, self.reserve1)
        } else {
            (self.reserve1, self.reserve0)
        }
    }
}
