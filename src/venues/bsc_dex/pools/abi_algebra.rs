//! Файл: venues/bsc_dex/pools/abi_algebra.rs
//! Что делает: ABI пула и фабрики Algebra (THENA Fusion и другие форки Algebra V1/V1.9, решения 80, 81) — для eth_call
//! чтения состояния (load_algebra.rs), проверки пула (verify_algebra.rs) и разбора событий (events_algebra.rs).
//! Кто вызывает: pools/load_algebra.rs, pools/events_algebra.rs, pools/verify_algebra.rs, pools/created_log.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адрес фабрики — dex.factory по виду пула; адрес пула — из справочника).
//!
//! Сигнатуры — из официального пакета @cryptoalgebra/v1.9-core (= линия THENA Fusion; решение 23):
//! - пул (IAlgebraPoolState.sol): globalState() → (uint160 price, int24 tick, uint16 fee, uint16 timepointIndex,
//!   uint8 communityFeeToken0, uint8 communityFeeToken1, bool unlocked); liquidity() → uint128; tickSpacing() → int24;
//!   ticks(int24) → (uint128 liquidityTotal, int128 liquidityDelta, uint256 outerFeeGrowth0Token,
//!   uint256 outerFeeGrowth1Token, int56 outerTickCumulative, uint160 outerSecondsPerLiquidity,
//!   uint32 outerSecondsSpent, bool initialized); tickTable(int16 wordPosition) → uint256 (битмап СЫРОГО тика:
//!   rowNumber = tick >> 8, bitNumber = tick & 0xFF — TickTable.sol, БЕЗ деления на tickSpacing, в отличие от V3);
//! - события (IAlgebraPoolEvents.sol): Swap(sender, recipient, amount0, amount1, price, liquidity, tick),
//!   Mint(sender, owner, bottomTick, topTick, liquidityAmount, amount0, amount1),
//!   Burn(owner, bottomTick, topTick, liquidityAmount, amount0, amount1) — Swap несёт цену/ликвидность/тик после обмена;
//! - фабрика (IAlgebraFactory.sol): poolByPair(tokenA, tokenB) → address (ОДИН пул на пару — комиссия динамическая,
//!   уровней комиссии нет) и событие Pool(token0, token1, pool) создания нового пула.

// Код функций и событий генерирует alloy::sol! — число аргументов и полей задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    interface IAlgebraPool {
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 price, uint128 liquidity, int24 tick);
        event Mint(address sender, address indexed owner, int24 indexed bottomTick, int24 indexed topTick, uint128 liquidityAmount, uint256 amount0, uint256 amount1);
        event Burn(address indexed owner, int24 indexed bottomTick, int24 indexed topTick, uint128 liquidityAmount, uint256 amount0, uint256 amount1);
        function globalState() external view returns (uint160 price, int24 tick, uint16 fee, uint16 timepointIndex, uint8 communityFeeToken0, uint8 communityFeeToken1, bool unlocked);
        function liquidity() external view returns (uint128);
        function tickSpacing() external view returns (int24);
        function ticks(int24 tick) external view returns (uint128 liquidityTotal, int128 liquidityDelta, uint256 outerFeeGrowth0Token, uint256 outerFeeGrowth1Token, int56 outerTickCumulative, uint160 outerSecondsPerLiquidity, uint32 outerSecondsSpent, bool initialized);
        function tickTable(int16 wordPosition) external view returns (uint256);
    }

    interface IAlgebraFactory {
        event Pool(address indexed token0, address indexed token1, address pool);
        function poolByPair(address tokenA, address tokenB) external view returns (address pool);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};
    use alloy_primitives::{B256, keccak256};

    /// topic0 и селекторы совпадают с keccak256 канонических сигнатур официального контракта (@cryptoalgebra/v1.9-core).
    #[test]
    fn signatures_match_official() {
        let topic = |sig: &str| B256::from(keccak256(sig.as_bytes()));
        assert_eq!(
            IAlgebraPool::Swap::SIGNATURE_HASH,
            topic("Swap(address,address,int256,int256,uint160,uint128,int24)")
        );
        assert_eq!(
            IAlgebraPool::Mint::SIGNATURE_HASH,
            topic("Mint(address,address,int24,int24,uint128,uint256,uint256)")
        );
        assert_eq!(
            IAlgebraPool::Burn::SIGNATURE_HASH,
            topic("Burn(address,int24,int24,uint128,uint256,uint256)")
        );
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        assert_eq!(
            IAlgebraPool::globalStateCall::SELECTOR.to_vec(),
            sel("globalState()")
        );
        assert_eq!(
            IAlgebraPool::liquidityCall::SELECTOR.to_vec(),
            sel("liquidity()")
        );
        assert_eq!(
            IAlgebraPool::tickSpacingCall::SELECTOR.to_vec(),
            sel("tickSpacing()")
        );
        assert_eq!(
            IAlgebraPool::ticksCall::SELECTOR.to_vec(),
            sel("ticks(int24)")
        );
        assert_eq!(
            IAlgebraPool::tickTableCall::SELECTOR.to_vec(),
            sel("tickTable(int16)")
        );
        assert_eq!(
            IAlgebraFactory::poolByPairCall::SELECTOR.to_vec(),
            sel("poolByPair(address,address)")
        );
        assert_eq!(
            IAlgebraFactory::Pool::SIGNATURE_HASH,
            topic("Pool(address,address,address)")
        );
    }
}
