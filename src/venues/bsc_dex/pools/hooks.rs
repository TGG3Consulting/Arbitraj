//! Файл: venues/bsc_dex/pools/hooks.rs
//! Что делает: крючок пула V4 — флаги разрешений, влияющие на обмен, и группа A/B/C по решению 32.
//! Кто вызывает: pools/discover_v4.rs (при создании записи пула), позже quote/* и check/* (допуск по группе).
//! От чего зависит: core/types (HookInfo, HookGroup), pools/key.rs.
//! Настройки: нет (допуск групп hooks.allowed_groups применяют quote/* и check/*).
//!
//! Раскладка флагов разная (research/flow/bsc_v4.md, раздел 3):
//! - Infinity — биты 0–13 поля parameters: beforeSwap 6, afterSwap 7, beforeSwapReturnsDelta 10, afterSwapReturnsDelta 11 —
//!   https://github.com/pancakeswap/infinity-core/blob/3c5ef1be1f71442d8bc60cc6fd11f9c677b714df/src/pool-cl/interfaces/ICLHooks.sol#L11-L24
//!   (Bin — те же номера для swap: src/pool-bin/interfaces/IBinHooks.sol#L10-L23).
//! - Uniswap V4 — младшие 14 бит адреса крючка: BEFORE_SWAP 1<<7, AFTER_SWAP 1<<6, BEFORE_SWAP_RETURNS_DELTA 1<<3,
//!   AFTER_SWAP_RETURNS_DELTA 1<<2 — https://github.com/Uniswap/v4-core/blob/bea7f6d1c11cd5dc4f0d5151ffa37a1c239feee6/src/libraries/Hooks.sol#L27-L47 .
//! - Динамическая комиссия: fee == 0x800000 (DYNAMIC_FEE_FLAG) — infinity-core src/libraries/LPFeeLibrary.sol#L19,
//!   v4-core src/libraries/LPFeeLibrary.sol#L15.
//!
//! Группа (решение 32): A — нет флагов обмена и комиссия не динамическая; C — включён любой ReturnsDelta;
//! B — остальное (динамическая комиссия или beforeSwap/afterSwap без ReturnsDelta).

use alloy_primitives::{Address, U160};

use crate::core::types::{HookGroup, HookInfo};

use super::key::{PoolKey, infinity_hook_bits};

/// Флаг динамической комиссии в PoolKey.fee.
pub const DYNAMIC_FEE_FLAG: u32 = 0x80_0000;
/// Маска 14 бит флагов (ALL_HOOK_MASK у Uniswap; у Infinity — биты 0–13).
const ALL_HOOK_MASK: u16 = (1 << 14) - 1;

/// Номера битов флагов, влияющих на обмен.
struct Bits {
    before_swap: u16,
    after_swap: u16,
    before_delta: u16,
    after_delta: u16,
}

const INFINITY: Bits = Bits {
    before_swap: 6,
    after_swap: 7,
    before_delta: 10,
    after_delta: 11,
};
const UNISWAP: Bits = Bits {
    before_swap: 7,
    after_swap: 6,
    before_delta: 3,
    after_delta: 2,
};

/// Крючок пула по его ключу. Ошибок нет.
pub fn info(key: &PoolKey) -> HookInfo {
    let (raw, bits) = match key {
        PoolKey::Infinity(k) => (infinity_hook_bits(k.parameters) & ALL_HOOK_MASK, &INFINITY),
        PoolKey::Uniswap(k) => (uniswap_bits(k.hooks), &UNISWAP),
    };
    let on = |b: u16| raw & (1 << b) != 0;
    let dynamic_fee = key.fee() == DYNAMIC_FEE_FLAG;
    let (before_swap, after_swap) = (on(bits.before_swap), on(bits.after_swap));
    let (before_delta, after_delta) = (on(bits.before_delta), on(bits.after_delta));
    let group = if before_delta || after_delta {
        HookGroup::C
    } else if dynamic_fee || before_swap || after_swap {
        HookGroup::B
    } else {
        HookGroup::A
    };
    HookInfo {
        address: key.hooks(),
        group,
        dynamic_fee,
        before_swap,
        after_swap,
        before_swap_returns_delta: before_delta,
        after_swap_returns_delta: after_delta,
        raw_bits: raw,
    }
}

/// Младшие 14 бит адреса крючка Uniswap V4. Ошибок нет.
fn uniswap_bits(hooks: Address) -> u16 {
    let v = U160::from_be_slice(hooks.as_slice()) & U160::from(ALL_HOOK_MASK);
    v.to::<u16>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::bsc_dex::pools::key::tests::doc_key;

    /// Пул BNB/CAKE из документации PancakeSwap: флаги 0x00c2 (afterInitialize, beforeSwap, afterSwap),
    /// динамическая комиссия → группа B.
    #[test]
    fn doc_pool_is_group_b() {
        let h = info(&PoolKey::Infinity(doc_key()));
        assert_eq!(h.raw_bits, 0x00c2);
        assert!(h.before_swap && h.after_swap && h.dynamic_fee);
        assert!(!h.before_swap_returns_delta && !h.after_swap_returns_delta);
        assert_eq!(h.group, HookGroup::B);
    }
}
