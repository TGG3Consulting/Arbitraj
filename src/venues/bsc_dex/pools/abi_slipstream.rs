//! Файл: venues/bsc_dex/pools/abi_slipstream.rs
//! Что делает: ABI пула и фабрики Slipstream (Topaz v3 на BSC, решения 80, 85) — для eth_call чтения состояния
//! (load_slipstream.rs), проверки пула (verify*.rs: реестр фабрики getPool по tickSpacing) и дискавери новых пулов
//! (created_log.rs: событие PoolCreated). Swap/Mint/Burn НЕ объявляются здесь: Slipstream — построчный форк Uniswap V3
//! core, его события пула байт-в-байт совпадают с Uniswap V3, поэтому разбор берётся из abi_v2v3 (IUniswapV3Pool::Swap,
//! IV3Pool::Mint/Burn) через events_v3.rs. Два отличия Slipstream от Uniswap V3, из-за которых нужен отдельный модуль:
//! (1) ключ пула фабрики — int24 tickSpacing (а НЕ uint24 fee), getPool(a, b, int24) и PoolCreated(…, int24 indexed
//! tickSpacing, pool); (2) slot0 без поля feeProtocol (6 полей, не 7 как у Uniswap V3) — свой разбор slot0; плюс
//! getter ticks() форка возвращает 10 полей (добавлены stakedLiquidityNet, rewardGrowthOutsideX128) — liquidityGross
//! и liquidityNet по-прежнему первые два, остальное для цены свопа — шум. Комиссия — с КАЖДОГО пула fee() (у форка
//! может быть переопределена владельцем фабрики или волатильностным модулем), не из таблицы дефолтов по tickSpacing.
//! Кто вызывает: pools/load_slipstream.rs, pools/verify_calls.rs, pools/created_log.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адрес — dex.factory по виду пула).
//!
//! Сигнатуры — из официального публичного исходника Slipstream velodrome-finance/slipstream (HEAD
//! 193a6b85f836a229b76a8df3541a284a2368427b, решение 23):
//! - фабрика getPool(address, address, int24 tickSpacing) и событие PoolCreated(token0, token1, tickSpacing, pool) —
//!   contracts/core/CLFactory.sol#L217, contracts/core/interfaces/ICLFactory.sol#L45,#L120 (у фабрики две перегрузки
//!   getPool — int24 и uint24; канонический ключ Slipstream — int24 tickSpacing);
//! - пул slot0/liquidity/fee/tickSpacing/tickBitmap/ticks — contracts/core/CLPool.sol#L53-L112,#L181
//!   (slot0: sqrtPriceX96, tick, observationIndex, observationCardinality, observationCardinalityNext, unlocked — БЕЗ
//!   feeProtocol; fee() = ICLFactory.getSwapFee(pool); ticks — getter mapping(int24 => Tick.Info), Tick.Info —
//!   contracts/core/libraries/Tick.sol struct Info: 10 полей, liquidityGross/liquidityNet первыми);
//! - Swap/Mint/Burn пула (НЕ здесь, см. abi_v2v3) — contracts/core/interfaces/pool/ICLPoolEvents.sol#L72 (Swap из 7
//!   полей, идентичен Uniswap V3: Swap(address,address,int256,int256,uint160,uint128,int24)).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    interface ICLPool {
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, bool unlocked);
        function liquidity() external view returns (uint128);
        function fee() external view returns (uint24);
        function tickSpacing() external view returns (int24);
        function tickBitmap(int16 wordPosition) external view returns (uint256);
        function ticks(int24 tick) external view returns (uint128 liquidityGross, int128 liquidityNet, int128 stakedLiquidityNet, uint256 feeGrowthOutside0X128, uint256 feeGrowthOutside1X128, uint256 rewardGrowthOutsideX128, int56 tickCumulativeOutside, uint160 secondsPerLiquidityOutsideX128, uint32 secondsOutside, bool initialized);
    }

    interface ICLFactory {
        function getPool(address tokenA, address tokenB, int24 tickSpacing) external view returns (address pool);
        event PoolCreated(address indexed token0, address indexed token1, int24 indexed tickSpacing, address pool);
    }
}

#[cfg(test)]
mod tests {
    use super::super::abi_v2v3::IUniswapV3Pool;
    use super::*;
    use alloy::sol_types::{SolCall, SolEvent};
    use alloy_primitives::{B256, keccak256};

    /// topic0 и селекторы совпадают с keccak256 канонических сигнатур официального Slipstream-исходника, и событие
    /// Swap пула идентично Uniswap V3 (поэтому разбор событий переиспользуется из abi_v2v3/events_v3).
    #[test]
    fn signatures_match_official() {
        let topic = |sig: &str| B256::from(keccak256(sig.as_bytes()));
        let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
        // Фабрика: ключ пула — int24 tickSpacing (не uint24 fee как у Uniswap V3).
        assert_eq!(
            ICLFactory::getPoolCall::SELECTOR.to_vec(),
            sel("getPool(address,address,int24)")
        );
        assert_eq!(
            ICLFactory::PoolCreated::SIGNATURE_HASH,
            topic("PoolCreated(address,address,int24,address)")
        );
        // Пул: slot0 без feeProtocol (6 полей), fee/tickSpacing/liquidity/tickBitmap/ticks — геттеры форка Uni V3.
        assert_eq!(ICLPool::slot0Call::SELECTOR.to_vec(), sel("slot0()"));
        assert_eq!(ICLPool::feeCall::SELECTOR.to_vec(), sel("fee()"));
        assert_eq!(
            ICLPool::tickSpacingCall::SELECTOR.to_vec(),
            sel("tickSpacing()")
        );
        assert_eq!(
            ICLPool::liquidityCall::SELECTOR.to_vec(),
            sel("liquidity()")
        );
        assert_eq!(
            ICLPool::tickBitmapCall::SELECTOR.to_vec(),
            sel("tickBitmap(int16)")
        );
        assert_eq!(ICLPool::ticksCall::SELECTOR.to_vec(), sel("ticks(int24)"));
        // Событие Swap Slipstream = Swap Uniswap V3 байт-в-байт → один topic0, разбор берётся из abi_v2v3.
        assert_eq!(
            IUniswapV3Pool::Swap::SIGNATURE_HASH,
            topic("Swap(address,address,int256,int256,uint160,uint128,int24)")
        );
    }
}
