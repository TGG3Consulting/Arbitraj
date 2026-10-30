//! Файл: venues/bsc_dex/math/mod.rs
//! Что делает: точная математика пулов — порты официальных библиотек контрактов на целых числах (U256),
//! с теми же округлениями, что в Solidity. Без запросов к узлу.
//! Кто вызывает: pools/* (цена бина для глубины), quote/* (шаг 1c).
//! От чего зависит: alloy-primitives.
//! Настройки: нет.

pub mod bin_price;
pub mod bin_swap;
pub mod dodo_math;
pub mod full_math;
pub mod sqrt_price_math;
pub mod swap_algebra;
pub mod swap_cl;
pub mod swap_dodo;
pub mod swap_elastic;
pub mod swap_elastic_step;
pub mod swap_step;
pub mod tick_math;
pub mod v2;
