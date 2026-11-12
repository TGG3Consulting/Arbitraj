//! Файл: venues/bsc_dex/node/block_ms.rs
//! Что делает: время блока BSC в миллисекундах (решение 259). В заголовке BSC секунды — поле timestamp, а доля
//! секунды (0–999 мс) — в поле mixHash (BEP-520, хардфорк Lorentz: «The millisecond component is stored in the last
//! two bytes of Header.MixDigest»; запись — uint256(milliseconds % 1000) в 32 байта, big-endian; MilliTimestamp() =
//! Time·1000 + milliseconds; «Ensure Milliseconds is less than 1000 when verifying the block header»).
//! Без миллисекунд возраст блока был завышен на 0–999 мс (head_age.rs). Время для deadline и симуляции (секунды —
//! block.timestamp в EVM) этим не меняется: оно по-прежнему в секундах (heads.rs at, block_time).
//! Кто вызывает: node/head_age.rs (fetch), connection_sync.rs (check).
//! От чего зависит: ничего, кроме alloy_primitives.
//! Настройки: нет.
//!
//! Источник: BEP-520 — https://github.com/bnb-chain/BEPs/blob/master/BEPs/BEP-520.md (раздел Specification). Живой
//! ответ узла (публичный RPC BNB Chain, блок 132983865, 12.11.2026): timestamp 0x6af47ad0, mixHash …0384 (900 мс),
//! milliTimestamp 0x1a1caffc004 = 0x6af47ad0·1000 + 900 — сверено в tests/dex_block_ms.rs.

use alloy_primitives::{B256, U256};

/// Что делает: время блока в миллисекундах Unix по секундам заголовка и полю mixHash.
/// Принимает: секунды (timestamp), mixHash (None — поля нет в ответе). Возвращает: секунды·1000 + доля секунды;
/// mixHash нет или его значение не меньше 1000 (блок до Lorentz, где mixHash — не миллисекунды) — доля 0, то есть
/// как раньше по одним секундам (возраст не занижается). Ошибок нет.
pub fn millis(secs: u64, mix: Option<B256>) -> u64 {
    let part = mix
        .map(|h| U256::from_be_bytes(h.0))
        .filter(|v| *v < U256::from(1000u64))
        .map_or(0, |v| v.to::<u64>());
    secs.saturating_mul(1000).saturating_add(part)
}
