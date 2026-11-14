//! Файл: core/types/pool_solidly.rs
//! Что делает: комиссия и ограничения обмена пула вида Solidly (решение 273) — без названия площадки: число комиссии
//! с контракта и ПРАВИЛО, по которому контракт вычитает её из входа (числитель над делителем или делитель), плюс
//! ограничения обмена, которые проверяет сам контракт пары (пауза фабрики, предел цены газа транзакции). Вынесено из
//! pool.rs (предел размера файла).
//! Кто вызывает: core/types/pool.rs (PoolState::Solidly), адаптер DEX (pools/load_solidly.rs создаёт,
//! math/solidly_fee.rs применяет, quote/* и exec/* проверяют ограничения).
//! Ревью 273: записи до решения 273 (эпизоды, окна, снимки) несут `"fee": число` — числитель над GETFEE_DENOMINATOR
//! (прежний смысл поля); читаются оба вида, пишется новый `{value, rule}`.
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

/// Делитель числителя комиссии getFee Velodrome / THENA V1 (контракт: `amountIn * fee / 10000`) — и смысл поля `fee`
/// в записях до решения 273. Единственный источник числа: адаптер берёт его же (math/solidly_math.rs fee_denominator).
pub const GETFEE_DENOMINATOR: u32 = 10_000;

/// Комиссия пула Solidly: число с контракта и правило его применения. Чтение — оба вида записи (FeeRecord).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "FeeRecord")]
pub struct SolidlyFee {
    /// Значение с контракта как есть (getFee / fee() / swapFee() / stableFee() …).
    pub value: u32,
    pub rule: FeeRule,
}

/// Вид записи комиссии при чтении (ревью 273): до решения 273 — число (числитель над GETFEE_DENOMINATOR), после —
/// `{value, rule}`.
#[derive(Deserialize)]
#[serde(untagged)]
enum FeeRecord {
    Legacy(u32),
    Current { value: u32, rule: FeeRule },
}

impl From<FeeRecord> for SolidlyFee {
    /// Запись → комиссия: число — правило «числитель над GETFEE_DENOMINATOR» (как его применял код до решения 273).
    fn from(r: FeeRecord) -> Self {
        match r {
            FeeRecord::Legacy(value) => SolidlyFee {
                value,
                rule: FeeRule::Numerator {
                    denominator: GETFEE_DENOMINATOR,
                },
            },
            FeeRecord::Current { value, rule } => SolidlyFee { value, rule },
        }
    }
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
