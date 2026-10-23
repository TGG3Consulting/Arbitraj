//! Файл: venues/bsc_dex/pools/key.rs
//! Что делает: ключ пула V4 (PoolKey) PancakeSwap Infinity и Uniswap V4, расчёт PoolId и разбор поля
//! parameters Infinity (флаги крючка, tickSpacing у CL, binStep у Bin). Ключ нужен для обмена (exec) и квотеров.
//! Кто вызывает: pools/discover_v4.rs (ключ из события Initialize), pools/hooks.rs, позже quote/*, exec/v4.rs.
//! От чего зависит: alloy::sol!, alloy-primitives (keccak256).
//! Настройки: нет.
//!
//! Источники (research/flow/bsc_v4.md, раздел 2):
//! - Infinity PoolKey (6 полей: currency0, currency1, hooks, poolManager, fee, parameters) —
//!   https://github.com/pancakeswap/infinity-core/blob/3c5ef1be1f71442d8bc60cc6fd11f9c677b714df/src/types/PoolKey.sol#L12-L25 ;
//!   PoolId = keccak256(poolKey, 0xc0) — src/types/PoolId.sol#L10-L14.
//! - parameters: биты 0–15 — флаги крючка (src/libraries/math/ParametersHelper.sol, OFFSET_HOOK = 0);
//!   CL: биты 16–39 — tickSpacing (src/pool-cl/libraries/CLPoolParametersHelper.sol, OFFSET_TICK_SPACING = 16);
//!   Bin: биты 16–31 — binStep (src/pool-bin/libraries/BinPoolParametersHelper.sol, OFFSET_BIN_STEP = 16).
//! - Uniswap V4 PoolKey (5 полей: currency0, currency1, fee, tickSpacing, hooks) —
//!   https://github.com/Uniswap/v4-core/blob/bea7f6d1c11cd5dc4f0d5151ffa37a1c239feee6/src/types/PoolKey.sol#L11-L22 ;
//!   PoolId = keccak256(poolKey, 0xa0) — src/types/PoolId.sol.

use alloy::sol_types::SolValue;
use alloy_primitives::aliases::{I24, U24};
use alloy_primitives::{Address, B256, U256, keccak256};

alloy::sol! {
    /// PoolKey PancakeSwap Infinity.
    struct InfinityPoolKey { address currency0; address currency1; address hooks; address poolManager; uint24 fee; bytes32 parameters; }
    /// PoolKey Uniswap V4.
    struct UniswapPoolKey { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
}

/// Сдвиг флагов крючка в parameters Infinity (OFFSET_HOOK).
const OFFSET_HOOK: usize = 0;
/// Сдвиг tickSpacing (CL) и binStep (Bin) в parameters Infinity.
const OFFSET_SPACING: usize = 16;

/// Ключ пула V4 — как его требует контракт своей DEX.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolKey {
    Infinity(InfinityPoolKeyData),
    Uniswap(UniswapPoolKeyData),
}

/// Поля ключа Infinity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfinityPoolKeyData {
    pub currency0: Address,
    pub currency1: Address,
    pub hooks: Address,
    pub manager: Address,
    pub fee: u32,
    pub parameters: B256,
}

/// Поля ключа Uniswap V4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniswapPoolKeyData {
    pub currency0: Address,
    pub currency1: Address,
    pub fee: u32,
    pub tick_spacing: i32,
    pub hooks: Address,
}

impl PoolKey {
    /// PoolId = keccak256(abi.encode(PoolKey)). Ошибок нет.
    pub fn id(&self) -> B256 {
        keccak256(self.abi())
    }

    /// ABI-кодирование ключа (для PoolId и для вызовов роутера/квотера). Ошибок нет.
    pub fn abi(&self) -> Vec<u8> {
        match self {
            PoolKey::Infinity(k) => InfinityPoolKey {
                currency0: k.currency0,
                currency1: k.currency1,
                hooks: k.hooks,
                poolManager: k.manager,
                fee: U24::from(k.fee),
                parameters: k.parameters,
            }
            .abi_encode(),
            PoolKey::Uniswap(k) => UniswapPoolKey {
                currency0: k.currency0,
                currency1: k.currency1,
                fee: U24::from(k.fee),
                tickSpacing: I24::try_from(k.tick_spacing).unwrap_or(I24::ZERO),
                hooks: k.hooks,
            }
            .abi_encode(),
        }
    }

    /// Адрес крючка (нулевой — крючка нет). Ошибок нет.
    pub fn hooks(&self) -> Address {
        match self {
            PoolKey::Infinity(k) => k.hooks,
            PoolKey::Uniswap(k) => k.hooks,
        }
    }

    /// Комиссия из ключа (единицы 1e-6; 0x800000 — динамическая). Ошибок нет.
    pub fn fee(&self) -> u32 {
        match self {
            PoolKey::Infinity(k) => k.fee,
            PoolKey::Uniswap(k) => k.fee,
        }
    }
}

/// Флаги крючка из parameters Infinity: биты 0–15. Ошибок нет.
pub fn infinity_hook_bits(parameters: B256) -> u16 {
    let v = U256::from_be_bytes(parameters.0) >> OFFSET_HOOK;
    (v & U256::from(0xffffu32)).to::<u16>()
}

/// tickSpacing Infinity CL: биты 16–39, приведение int24(uint24) как в контракте. Ошибок нет.
pub fn infinity_tick_spacing(parameters: B256) -> i32 {
    let raw = ((U256::from_be_bytes(parameters.0) >> OFFSET_SPACING) & U256::from(0xff_ffffu32))
        .to::<u32>();
    // int24(uint24): старший из 24 бит — знак.
    ((raw << 8) as i32) >> 8
}

/// binStep Infinity Bin: биты 16–31. Ошибок нет.
pub fn infinity_bin_step(parameters: B256) -> u32 {
    ((U256::from_be_bytes(parameters.0) >> OFFSET_SPACING) & U256::from(0xffffu32)).to::<u32>()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Пример из официальной документации PancakeSwap: parameters = 0x…0a00c2 → флаги 0x00c2, tickSpacing 10
    /// (developer.pancakeswap.finance, «PancakeSwap Infinity vs Uniswap v4», research/flow/bsc_v4.md, раздел 2).
    #[test]
    fn infinity_parameters_doc_example() {
        let mut p = [0u8; 32];
        p[29] = 0x0a;
        p[30] = 0x00;
        p[31] = 0xc2;
        let p = B256::from(p);
        assert_eq!(infinity_hook_bits(p), 0x00c2);
        assert_eq!(infinity_tick_spacing(p), 10);
    }

    /// Реальный пул BNB/CAKE из той же страницы документации: ключ (ответ poolIdToPoolKey) → PoolId
    /// 0x737a7d974a19bafb34c8d74d898188c9b59689b91f291fa6ade69f71fa0f5afa.
    #[test]
    fn infinity_pool_id_doc_example() {
        let key = PoolKey::Infinity(doc_key());
        assert_eq!(
            key.id().to_string(),
            "0x737a7d974a19bafb34c8d74d898188c9b59689b91f291fa6ade69f71fa0f5afa"
        );
    }

    /// Ключ пула BNB/CAKE из документации PancakeSwap (pancakeswap-infinity-vs-uniswap-v4.mdx#L71-L86).
    pub fn doc_key() -> InfinityPoolKeyData {
        InfinityPoolKeyData {
            currency0: Address::ZERO,
            currency1: "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82"
                .parse()
                .unwrap(),
            hooks: "0x32C59D556B16DB81DFc32525eFb3CB257f7e493d"
                .parse()
                .unwrap(),
            manager: "0xa0FfB9c1CE1Fe56963B0321B32E7A0302114058b"
                .parse()
                .unwrap(),
            fee: 8_388_608,
            parameters: "0x00000000000000000000000000000000000000000000000000000000000a00c2"
                .parse()
                .unwrap(),
        }
    }
}
