//! Файл: venues/bsc_dex/pools/abi_v2v3.rs
//! Что делает: ABI фабрик и пулов V2 и V3 (PancakeSwap и Uniswap) и TickLens — для eth_call и разбора событий.
//! Кто вызывает: pools/discover_v2.rs, discover_v3.rs, load_v2v3.rs, events_v2.rs, events_v3.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет.
//!
//! Сигнатуры — из официальных исходников (research/flow/bsc_dex.md, разделы 2–4):
//! - V2 фабрика getPair, PairCreated; пара getReserves, Sync —
//!   https://github.com/Uniswap/v2-core/blob/6a9e7c97860676e0992f22a49665760444c1cdf5/contracts/interfaces/IUniswapV2Factory.sol ,
//!   https://github.com/Uniswap/v2-core/blob/6a9e7c97860676e0992f22a49665760444c1cdf5/contracts/interfaces/IUniswapV2Pair.sol
//!   (у PancakeSwap те же: https://github.com/pancakeswap/pancake-swap-core/blob/3b214306770e86bc3a64e67c2b5bdb566b4e94a7/contracts/interfaces/IPancakeFactory.sol#L4-L11 ).
//! - V3 фабрика getPool, feeAmountTickSpacing — IPancakeV3Factory.sol#L34-L70 (pancake-v3-contracts 98684794),
//!   https://github.com/Uniswap/v3-core/blob/5ea68a77cb541c6b3812df783be71ba8c081e25f/contracts/interfaces/IUniswapV3Factory.sol#L37-L54 .
//! - V3 PoolCreated, Mint, Burn — IPancakeV3Factory.sol#L23-L29, IPancakeV3PoolEvents.sol#L21-L62 (pancake-v3-contracts 98684794);
//!   Swap PancakeSwap (9 полей) — IPancakeV3PoolEvents.sol#L74-L84; Swap Uniswap (7 полей) —
//!   https://github.com/Uniswap/v3-core/blob/5ea68a77cb541c6b3812df783be71ba8c081e25f/contracts/interfaces/pool/IUniswapV3PoolEvents.sol#L72-L80 .
//! - slot0: у PancakeSwap feeProtocol uint32 (IPancakeV3PoolState.sol#L21-L32), у Uniswap uint8
//!   (IUniswapV3PoolState.sol#L21-L32) — свой разбор на DEX.
//! - TickLens.getPopulatedTicksInWord — pancake-v3-contracts/projects/v3-periphery/contracts/interfaces/ITickLens.sol#L6-L24
//!   (у Uniswap тот же интерфейс: v3-periphery/contracts/interfaces/ITickLens.sol).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    interface IV2Factory {
        function getPair(address tokenA, address tokenB) external view returns (address pair);
        event PairCreated(address indexed token0, address indexed token1, address pair, uint256 index);
    }

    interface IV2Pair {
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
        event Sync(uint112 reserve0, uint112 reserve1);
    }

    interface IV3Factory {
        function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address pool);
        function feeAmountTickSpacing(uint24 fee) external view returns (int24);
        event PoolCreated(address indexed token0, address indexed token1, uint24 indexed fee, int24 tickSpacing, address pool);
    }

    interface IV3Pool {
        function liquidity() external view returns (uint128);
        event Mint(address sender, address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 amount, uint256 amount0, uint256 amount1);
        event Burn(address indexed owner, int24 indexed tickLower, int24 indexed tickUpper, uint128 amount, uint256 amount0, uint256 amount1);
    }

    interface IPancakeV3Pool {
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint32 feeProtocol, bool unlocked);
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick, uint128 protocolFeesToken0, uint128 protocolFeesToken1);
    }

    interface IUniswapV3Pool {
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked);
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick);
    }

    interface ITickLens {
        struct PopulatedTick { int24 tick; int128 liquidityNet; uint128 liquidityGross; }
        function getPopulatedTicksInWord(address pool, int16 tickBitmapIndex) external view returns (PopulatedTick[] memory populatedTicks);
    }
}
