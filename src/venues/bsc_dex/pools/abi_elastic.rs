//! Файл: venues/bsc_dex/pools/abi_elastic.rs
//! Что делает: ABI пула, фабрики и читателя тиков KyberSwap Elastic (решение 81) — для eth_call чтения состояния
//! (load_elastic.rs), проверки пула (verify*.rs) и разбора событий (events_elastic.rs).
//! Кто вызывает: pools/load_elastic.rs, pools/events_elastic.rs, pools/verify_calls.rs, pools/verify_rules.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адреса — dex.factory, dex.tick_reader по виду пула).
//!
//! Сигнатуры — из официальных исходников KyberNetwork/ks-elastic-sc (HEAD 4ab08c0, решение 23):
//! - пул: getPoolState, getLiquidityState, swapFeeUnits, tickDistance, ticks, initializedTicks —
//!   contracts/interfaces/pool/IPoolStorage.sol#L28-L101;
//! - события Swap, Mint, Burn, BurnRTokens — contracts/interfaces/pool/IPoolEvents.sol#L20-L71 (Swap несёт baseL
//!   после обмена, НЕ reinvestL);
//! - фабрика getPool(tokenA, tokenB, swapFeeUnits) и событие PoolCreated(token0, token1, swapFeeUnits, tickDistance,
//!   pool) — contracts/interfaces/IFactory.sol#L13-L99 (порядок токенов не важен);
//! - читатель тиков getTicksInRange(pool, startTick, length) → int24[] (length = 0 — все инициализированные тики
//!   вверх от startTick по связному списку initializedTicks) — contracts/periphery/TicksFeesReader.sol#L37-L67.

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    interface IElasticPool {
        event Swap(address indexed sender, address indexed recipient, int256 deltaQty0, int256 deltaQty1, uint160 sqrtP, uint128 liquidity, int24 currentTick);
        event Mint(address sender, address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 qty, uint256 qty0, uint256 qty1);
        event Burn(address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 qty, uint256 qty0, uint256 qty1);
        event BurnRTokens(address indexed owner, uint256 qty, uint256 qty0, uint256 qty1);
        function swapFeeUnits() external view returns (uint24);
        function tickDistance() external view returns (int24);
        function getPoolState() external view returns (uint160 sqrtP, int24 currentTick, int24 nearestCurrentTick, bool locked);
        function getLiquidityState() external view returns (uint128 baseL, uint128 reinvestL, uint128 reinvestLLast);
        function ticks(int24 tick) external view returns (uint128 liquidityGross, int128 liquidityNet, uint256 feeGrowthOutside, uint128 secondsPerLiquidityOutside);
    }

    interface IElasticFactory {
        event PoolCreated(address indexed token0, address indexed token1, uint24 indexed swapFeeUnits, int24 tickDistance, address pool);
        function getPool(address tokenA, address tokenB, uint24 swapFeeUnits) external view returns (address pool);
    }

    interface ITicksFeesReader {
        function getTicksInRange(address pool, int24 startTick, uint32 length) external view returns (int24[] memory allTicks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};
    use alloy_primitives::{B256, keccak256};

    /// topic0 и селекторы совпадают с keccak256 канонических сигнатур официального контракта (ks-elastic-sc).
    #[test]
    fn signatures_match_official() {
        let topic = |sig: &str| B256::from(keccak256(sig.as_bytes()));
        assert_eq!(
            IElasticPool::Swap::SIGNATURE_HASH,
            topic("Swap(address,address,int256,int256,uint160,uint128,int24)")
        );
        assert_eq!(
            IElasticPool::Mint::SIGNATURE_HASH,
            topic("Mint(address,address,int24,int24,uint128,uint256,uint256)")
        );
        assert_eq!(
            IElasticPool::Burn::SIGNATURE_HASH,
            topic("Burn(address,int24,int24,uint128,uint256,uint256)")
        );
        assert_eq!(
            IElasticPool::BurnRTokens::SIGNATURE_HASH,
            topic("BurnRTokens(address,uint256,uint256,uint256)")
        );
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        assert_eq!(
            IElasticPool::getPoolStateCall::SELECTOR.to_vec(),
            sel("getPoolState()")
        );
        assert_eq!(
            IElasticPool::getLiquidityStateCall::SELECTOR.to_vec(),
            sel("getLiquidityState()")
        );
        assert_eq!(
            IElasticPool::swapFeeUnitsCall::SELECTOR.to_vec(),
            sel("swapFeeUnits()")
        );
        assert_eq!(
            IElasticFactory::getPoolCall::SELECTOR.to_vec(),
            sel("getPool(address,address,uint24)")
        );
        assert_eq!(
            ITicksFeesReader::getTicksInRangeCall::SELECTOR.to_vec(),
            sel("getTicksInRange(address,int24,uint32)")
        );
    }
}
