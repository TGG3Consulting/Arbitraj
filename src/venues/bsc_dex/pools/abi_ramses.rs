//! Файл: venues/bsc_dex/pools/abi_ramses.rs
//! Что делает: решение 483 — ABI того, чем пулы Ramses (сеть Arbitrum) отличаются от видов, вариантами которых они
//! сделаны (pools/family_variant.rs: RamsesCl, RamsesClV2, RamsesLegacy): slot0 и ticks() обеих CL-линий, currentFee()
//! старой линии, событие пула FeeAdjustment, Mint линии V3 (с полем index). Остальное (liquidity, tickBitmap, fee(),
//! Swap / Burn, getPool, PoolCreated, Sync / PairCreated легаси) — общие ABI (abi_slipstream.rs, abi_v2v3.rs,
//! abi_solidly.rs), подписи совпадают — сверено тестом ниже.
//! Кто вызывает: pools/load_ramses.rs (чтение), pools/ramses.rs (события, создание пулов), тесты.
//! От чего зависит: alloy::sol!.
//! Настройки: нет.
//!
//! Исходники (Sourcify, сеть 42161, проверено 20.11.2026 через sourcify.dev/server/v2/contract):
//! - RamsesV2Pool (реализация за RamsesBeaconProxy) 0x6553aAEE5a3A482A7d61bb5e093B05140Fe17e21 — match, solc 0.7.6:
//!   contracts/v2/RamsesV2Pool.sol — fee() L199-L201 (initialFee), currentFee() L216-L218 (states.fee), slot0
//!   L239-L263 (uint8 feeProtocol), ticks L338-L370 (10 полей); contracts/v2/libraries/ProtocolActions.sol —
//!   event FeeAdjustment(uint24 oldFee, uint24 newFee) L26, setFee L75-L85 (emit L84);
//!   contracts/v2/interfaces/pool/IRamsesV2PoolEvents.sol — Mint L21-L29, Burn L55-L62, Swap L72-L80 (= Uniswap V3).
//! - RamsesV3Pool (исходник в составе RamsesV3PoolDeployer 0xb722efaAbe807FAeA16068f595EaA9aa1a62CECD — match, solc
//!   0.8.28): contracts/CL/core/RamsesV3Pool.sol — fee() L798-L800 ($.fee), slot0 L817-L842 (uint24 feeProtocol),
//!   ticks L896-L920 (8 полей), emit Mint L307; contracts/CL/core/libraries/ProtocolActions.sol — event FeeAdjustment
//!   L27, setFee L80-L87 (emit L86); contracts/CL/core/interfaces/pool/IRamsesV3PoolEvents.sol — Mint L22-L31 (поле
//!   `uint256 index` третьим), Burn L57-L64, Swap L74-L82 (= Uniswap V3).
//! - RamsesV3Factory 0xd0019e86edB35E1fedaaB03aED5c3c60f115d28b — match: IRamsesV3Factory.sol PoolCreated L13-L15
//!   (подпись Uniswap V3 — IV3Factory::PoolCreated), getPool(a, b, int24) L62 (= ICLFactory::getPool).
//! - PairFactory 0xADd32480630A16dfAcEe6eeFcB3ab2181449Dc3B — match, solc 0.8.30: contracts/interfaces/IPair.sol
//!   Sync(uint112, uint112) L15 (= IV2Pair::Sync), IPairFactory.sol PairCreated(address indexed, address indexed,
//!   address, uint256) L5 (= IV2Factory::PairCreated).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    /// Старая CL-линия Ramses (RamsesV2Pool).
    interface IRamsesClPool {
        function currentFee() external view returns (uint24);
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked);
        function ticks(int24 tick) external view returns (uint128 liquidityGross, int128 liquidityNet, uint128 boostedLiquidityGross, int128 boostedLiquidityNet, uint256 feeGrowthOutside0X128, uint256 feeGrowthOutside1X128, int56 tickCumulativeOutside, uint160 secondsPerLiquidityOutsideX128, uint32 secondsOutside, bool initialized);
    }

    /// Линия V3 Ramses (RamsesV3Pool).
    interface IRamsesV3Pool {
        function fee() external view returns (uint24);
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint24 feeProtocol, bool unlocked);
        function ticks(int24 tick) external view returns (uint128 liquidityGross, int128 liquidityNet, uint256 feeGrowthOutside0X128, uint256 feeGrowthOutside1X128, int56 tickCumulativeOutside, uint160 secondsPerLiquidityOutsideX128, uint32 secondsOutside, bool initialized);
        event Mint(address sender, address indexed owner, uint256 index, int24 indexed tickLower, int24 indexed tickUpper, uint128 amount, uint256 amount0, uint256 amount1);
    }

    /// Событие смены комиссии пула — одинаково у обеих CL-линий (ProtocolActions.sol).
    interface IRamsesPoolFee {
        event FeeAdjustment(uint24 oldFee, uint24 newFee);
    }
}

#[cfg(test)]
mod tests {
    use super::super::abi_slipstream::{ICLFactory, ICLPool};
    use super::super::abi_v2v3::{IUniswapV3Pool, IV2Factory, IV2Pair, IV3Factory, IV3Pool};
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};
    use alloy_primitives::{B256, keccak256};

    /// Подписи — keccak256 канонических строк исходников (шапка); общие ABI совпадают с подписями Ramses.
    #[test]
    fn signatures_match_sources() {
        let topic = |sig: &str| B256::from(keccak256(sig.as_bytes()));
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        assert_eq!(
            IRamsesClPool::currentFeeCall::SELECTOR.to_vec(),
            sel("currentFee()")
        );
        assert_eq!(IRamsesClPool::slot0Call::SELECTOR.to_vec(), sel("slot0()"));
        assert_eq!(
            IRamsesClPool::ticksCall::SELECTOR.to_vec(),
            sel("ticks(int24)")
        );
        assert_eq!(IRamsesV3Pool::feeCall::SELECTOR.to_vec(), sel("fee()"));
        assert_eq!(
            IRamsesV3Pool::ticksCall::SELECTOR.to_vec(),
            sel("ticks(int24)")
        );
        assert_eq!(
            IRamsesV3Pool::Mint::SIGNATURE_HASH,
            topic("Mint(address,address,uint256,int24,int24,uint128,uint256,uint256)")
        );
        assert_ne!(
            IRamsesV3Pool::Mint::SIGNATURE_HASH,
            IV3Pool::Mint::SIGNATURE_HASH
        );
        assert_eq!(
            IRamsesPoolFee::FeeAdjustment::SIGNATURE_HASH,
            topic("FeeAdjustment(uint24,uint24)")
        );
        // Общие ABI: Mint / Burn / Swap старой линии, Burn / Swap линии V3 — подписи Uniswap V3.
        let mint = "Mint(address,address,int24,int24,uint128,uint256,uint256)";
        assert_eq!(IV3Pool::Mint::SIGNATURE_HASH, topic(mint));
        let burn = "Burn(address,int24,int24,uint128,uint256,uint256)";
        assert_eq!(IV3Pool::Burn::SIGNATURE_HASH, topic(burn));
        let swap = "Swap(address,address,int256,int256,uint160,uint128,int24)";
        assert_eq!(IUniswapV3Pool::Swap::SIGNATURE_HASH, topic(swap));
        let created = "PoolCreated(address,address,uint24,int24,address)";
        assert_eq!(IV3Factory::PoolCreated::SIGNATURE_HASH, topic(created));
        let get_pool = "getPool(address,address,int24)";
        assert_eq!(ICLFactory::getPoolCall::SELECTOR.to_vec(), sel(get_pool));
        assert_eq!(
            ICLPool::liquidityCall::SELECTOR.to_vec(),
            sel("liquidity()")
        );
        assert_eq!(
            ICLPool::tickBitmapCall::SELECTOR.to_vec(),
            sel("tickBitmap(int16)")
        );
        // Легаси: Sync(uint112, uint112) и PairCreated без stable — подписи Uniswap V2.
        assert_eq!(
            IV2Pair::Sync::SIGNATURE_HASH,
            topic("Sync(uint112,uint112)")
        );
        let pair = "PairCreated(address,address,address,uint256)";
        assert_eq!(IV2Factory::PairCreated::SIGNATURE_HASH, topic(pair));
    }
}
