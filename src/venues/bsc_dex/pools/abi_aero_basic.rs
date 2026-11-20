//! Файл: venues/bsc_dex/pools/abi_aero_basic.rs
//! Что делает: решение 470 (T451-02) — ABI пулов Aero «Basic» (Aerodrome V2 под брендом Aero, сеть Arbitrum): пул
//! VolatilePool / StablePool, фабрика VolatilePoolFactory / StablePoolFactory (одна фабрика — одна кривая) и модуль
//! MEV-налога PriorityFeeMevTaxModule — для чтения состояния (load_aero_basic.rs), проверки пула (verify_calls.rs),
//! события создания (created_log.rs), поиска по фабрике (factory_ask.rs), квотера (quote/quoter_calls.rs).
//! Кто вызывает: pools/{load_aero_basic, verify_calls, created_log, factory_ask, aero_mev}.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет.
//!
//! Источник — Dromos-Labs/metadex-public, версия 1.1.0 (VERSIONS: коммит 79c8d5b62b7558c7fd58261523e517c588c0378d,
//! развёртывание arbitrum — deployment-addresses/pools-arbitrum.json; файлы ниже не менялись до 2ce1816):
//! - пул (V3/src/pools/Pool.sol): swap(uint256,uint256,address,bytes) L143 — как у Solidly/V2; комиссия обмена —
//!   factory.getFee(pool, msg.sender, amount0In, amount1In, r0, r1) L172, L389-L397; metadata() — 6 полей БЕЗ флага
//!   stable L461-L467 (у Velodrome — 7); getReserves() L530-L534 (три uint256); getAmountOut(uint256, address) L537-L545
//!   — комиссия getBaseFee(pool, address(0), …) без MEV-налога; factory/token0/token1 — public L49-L55; событие
//!   Sync(uint256, uint256) — V3/src/interfaces/pools/IPool.sol L48 (тот же topic0, что у Solidly);
//! - кривая — константа POOL_TYPE() (IPool.sol L204): 'V2_VOLATILE' (VolatilePool.sol L14) или 'V2_STABLE'
//!   (StablePool.sol L70);
//! - фабрика (V3/src/factories/PoolFactory.sol): getPool(tokenA, tokenB) L159-L161 (реестр в обе стороны L334-L335),
//!   isPool L164, isPaused L49 (пул откатывает swap на паузе — Pool.sol L144), getBaseFee(pool, caller, a0, a1, r0, r1)
//!   L348-L371, mevTaxModule L75; событие PoolCreated(address indexed token0, address indexed token1, address pool,
//!   uint256) — IPoolFactory.sol L53 (без stable, в отличие от Velodrome);
//! - модуль MEV-налога (V3/src/fees/PriorityFeeMevTaxModule.sol): priorityFeeMultiplier() L72 (uint64),
//!   minThreshold() L75 (uint96, wei).

alloy::sol! {
    interface IAeroBasicPool {
        function metadata() external view returns (uint256 dec0, uint256 dec1, uint256 r0, uint256 r1, address t0, address t1);
        function POOL_TYPE() external view returns (bytes32);
        function factory() external view returns (address);
        function getAmountOut(uint256 amountIn, address tokenIn) external view returns (uint256);
    }

    interface IAeroBasicFactory {
        event PoolCreated(address indexed token0, address indexed token1, address pool, uint256 allPoolsLength);

        function getPool(address tokenA, address tokenB) external view returns (address);
        function isPool(address pool) external view returns (bool);
        function isPaused() external view returns (bool);
        function getBaseFee(address pool, address caller, uint256 amount0In, uint256 amount1In, uint256 reserve0, uint256 reserve1) external view returns (uint256);
        function mevTaxModule() external view returns (address);
    }

    interface IAeroMevTaxModule {
        function minThreshold() external view returns (uint96);
        function priorityFeeMultiplier() external view returns (uint64);
    }
}

/// POOL_TYPE() стабильного пула: bytes32('V2_STABLE') — StablePool.sol L70. Ошибок нет.
pub fn stable_type() -> alloy_primitives::B256 {
    let mut b = [0u8; 32];
    b[..9].copy_from_slice(b"V2_STABLE");
    alloy_primitives::B256::from(b)
}

/// POOL_TYPE() волатильного пула: bytes32('V2_VOLATILE') — VolatilePool.sol L14. Ошибок нет.
pub fn volatile_type() -> alloy_primitives::B256 {
    let mut b = [0u8; 32];
    b[..11].copy_from_slice(b"V2_VOLATILE");
    alloy_primitives::B256::from(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};
    use alloy_primitives::{B256, keccak256};

    /// topic0 и селекторы — keccak256 сигнатур исходника metadex-public 1.1.0 (см. шапку).
    #[test]
    fn signatures_match_source() {
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        assert_eq!(
            IAeroBasicFactory::PoolCreated::SIGNATURE_HASH,
            B256::from(keccak256(b"PoolCreated(address,address,address,uint256)"))
        );
        assert_eq!(IAeroBasicPool::metadataCall::SELECTOR.to_vec(), sel("metadata()"));
        assert_eq!(IAeroBasicPool::POOL_TYPECall::SELECTOR.to_vec(), sel("POOL_TYPE()"));
        assert_eq!(
            IAeroBasicPool::getAmountOutCall::SELECTOR.to_vec(),
            sel("getAmountOut(uint256,address)")
        );
        assert_eq!(
            IAeroBasicFactory::getPoolCall::SELECTOR.to_vec(),
            sel("getPool(address,address)")
        );
        assert_eq!(
            IAeroBasicFactory::getBaseFeeCall::SELECTOR.to_vec(),
            sel("getBaseFee(address,address,uint256,uint256,uint256,uint256)")
        );
        assert_eq!(IAeroBasicFactory::isPausedCall::SELECTOR.to_vec(), sel("isPaused()"));
        assert_eq!(IAeroBasicFactory::mevTaxModuleCall::SELECTOR.to_vec(), sel("mevTaxModule()"));
        assert_eq!(IAeroMevTaxModule::minThresholdCall::SELECTOR.to_vec(), sel("minThreshold()"));
        assert_eq!(
            IAeroMevTaxModule::priorityFeeMultiplierCall::SELECTOR.to_vec(),
            sel("priorityFeeMultiplier()")
        );
        assert_eq!(&stable_type()[..9], b"V2_STABLE");
        assert_eq!(&volatile_type()[..11], b"V2_VOLATILE");
    }
}
