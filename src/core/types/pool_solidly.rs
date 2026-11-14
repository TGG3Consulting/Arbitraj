//! Файл: core/types/pool_solidly.rs
//! Что делает: комиссия и ограничения обмена пула вида Solidly (решение 273) — без названия площадки: число комиссии
//! с контракта и ПРАВИЛО, по которому контракт вычитает её из входа (числитель над делителем или делитель), плюс
//! ограничения обмена, которые проверяет сам контракт пары (пауза фабрики, предел цены газа транзакции). Вынесено из
//! pool.rs (предел размера файла).
//! Кто вызывает: core/types/pool.rs (PoolState::Solidly), адаптер DEX (pools/load_solidly.rs создаёт,
//! math/solidly_fee.rs применяет, quote/* и exec/* проверяют ограничения).
//! От чего зависит: serde.
//! Настройки: нет (правило задаёт адаптер по своим настройкам dex.solidly_fee_* [DEX]).

use serde::{Deserialize, Serialize};

/// Как контракт пары вычитает комиссию из входа (решение 273).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeeRule {
    /// `amountIn -= amountIn * fee / denominator` (Velodrome, THENA V1: делитель 10000; veplus, magicfox — тоже).
    Numerator { denominator: u32 },
    /// `amountIn -= amountIn / fee` (usdfi BasePair: fee — делитель; 100 → 1%, 10000 → 0,01%).
    Divisor,
}

/// Комиссия пула Solidly: число с контракта и правило его применения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolidlyFee {
    /// Значение с контракта как есть (getFee / fee() / swapFee() / stableFee() …).
    pub value: u32,
    pub rule: FeeRule,
}

/// Ограничения обмена, которые проверяет контракт пары (решение 273). По умолчанию — ограничений нет (DEX без них:
/// THENA V1, Topaz и др. — поведение как до решения 273).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SolidlyGate {
    /// Фабрика на паузе — обмен пары откатывается (usdfi: `require(!factory.isPaused())`).
    pub paused: bool,
    /// Предел цены газа транзакции обмена, wei (usdfi: `require(tx.gasprice <= factory.maxGasPrice())`). None — нет.
    pub max_gas_price: Option<u64>,
}
