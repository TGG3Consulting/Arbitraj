//! Файл: venues/bsc_dex/math/ekubo_config.rs
//! Что делает: разбор config ключа пула Ekubo (решение 447): расширение, комиссия 0.64, вид пула — концентрированная
//! ликвидность (шаг тика) или стейблсвап (усиление, центральный тик; усиление 0 и центр 0 — полный диапазон), диапазон
//! активной ликвидности стейблсвапа. Точный порт функций config ядра.
//! Кто вызывает: math/ekubo_swap.rs, pools/verify_ekubo.rs, pools/load_ekubo.rs, pools/events_ekubo.rs, exec (шаг EKUBO).
//! От чего зависит: alloy-primitives, math/ekubo_tick.rs (MIN_TICK, MAX_TICK).
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/types/poolConfig.sol: L71-L75 (fee: биты 32..95), L80-L84
//! (extension: старшие 160 бит), L97-L102 (tickSpacing: биты 0..30), L107-L112 (стейблсвап — бит 31 равен 0), L118-L123
//! (полный диапазон — младшие 32 бита нули), L129-L134 (усиление: биты 24..30), L141-L147 (центр: биты 0..23 со знаком ×
//! 16), L150-L156 (ширина = MAX_TICK >> усиление), L162-L173 (диапазон, обрезанный до MIN_TICK / MAX_TICK).

use alloy_primitives::{Address, B256};

use super::ekubo_tick::{MAX_TICK, MIN_TICK};

/// Вид пула по config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Концентрированная ликвидность с шагом тика.
    Concentrated { spacing: u32 },
    /// Полный диапазон (стейблсвап с усилением 0 и центром 0).
    FullRange,
    /// Стейблсвап: активная ликвидность в [lower, upper).
    Stable { lower: i32, upper: i32 },
}

/// Младшие 32 бита config (вид пула). Ошибок нет.
fn type_bits(c: &B256) -> u32 {
    u32::from_be_bytes([c[28], c[29], c[30], c[31]])
}

/// Комиссия пула (0.64, uint64). Ошибок нет.
pub fn fee(c: &B256) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&c[20..28]);
    u64::from_be_bytes(b)
}

/// Расширение пула (нулевой адрес — нет). Ошибок нет.
pub fn extension(c: &B256) -> Address {
    Address::from_slice(&c[..20])
}

/// Вид пула. Ошибок нет (config проверен ядром при создании пула — poolConfig.sol L249-L264).
pub fn shape(c: &B256) -> Shape {
    let t = type_bits(c);
    if t & 0x8000_0000 != 0 {
        return Shape::Concentrated {
            spacing: t & 0x7fff_ffff,
        };
    }
    if t == 0 {
        return Shape::FullRange;
    }
    let amp = (t >> 24) & 0x7f;
    // Центр: 24 бита со знаком (signextend(2, …)) × 16.
    let raw = (t & 0x00ff_ffff) as i32;
    let center = ((raw << 8) >> 8) * 16;
    let width = (MAX_TICK as u32).checked_shr(amp).unwrap_or(0) as i64;
    let lower = (i64::from(center) - width).max(i64::from(MIN_TICK)) as i32;
    let upper = (i64::from(center) + width).min(i64::from(MAX_TICK)) as i32;
    Shape::Stable { lower, upper }
}
