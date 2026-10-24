//! Файл: venues/bsc_dex/pools/abi_check.rs
//! Что делает: ABI для проверки пула из справочника индексатора на своём узле (решение 58): неизменяемые поля пулов
//! V2/V3 (factory, token0, token1, fee, tickSpacing), ключ пула у менеджеров PancakeSwap Infinity (poolIdToPoolKey)
//! и у PositionManager Uniswap V4 (poolKeys, poolManager).
//! Кто вызывает: pools/verify.rs, pools/verify_v4.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет.
//!
//! Сигнатуры — из официальных исходников:
//! - V2 пара factory()/token0()/token1() — https://github.com/Uniswap/v2-core/blob/6a9e7c97860676e0992f22a49665760444c1cdf5/contracts/interfaces/IUniswapV2Pair.sol#L37-L39 ;
//! - V3 пул factory/token0/token1/fee/tickSpacing — https://github.com/Uniswap/v3-core/blob/5ea68a77cb541c6b3812df783be71ba8c081e25f/contracts/interfaces/pool/IUniswapV3PoolImmutables.sol#L9-L28 ,
//!   PancakeSwap — pancake-v3-contracts 98684794 projects/v3-core/contracts/interfaces/pool/IPancakeV3PoolImmutables.sol#L9-L28
//!   (factory пула PancakeSwap — фабрика, не деплоер: PancakeV3Pool.sol#L129, parameters() деплоера);
//! - Infinity poolIdToPoolKey — https://github.com/pancakeswap/infinity-core/blob/3c5ef1be1f71442d8bc60cc6fd11f9c677b714df/src/interfaces/IPoolManager.sol#L31-L42
//!   (mapping в CLPoolManager.sol#L41 и BinPoolManager.sol#L48);
//! - Uniswap V4 PositionManager poolKeys(bytes25) — https://github.com/Uniswap/v4-periphery/blob/0c4e5bd3715ce2e72fd7609a1e2835f8300baf7b/src/PositionManager.sol#L126
//!   (ключ сохраняется при первой позиции, #L373-L380; bytes25 — первые 25 байт PoolId, PositionInfoLibrary.sol#L96),
//!   poolManager() — src/base/ImmutableState.sol#L11;
//! - lmPool() — только у PancakeSwap V3: PancakeV3Pool.sol#L106 (pancake-v3-contracts 98684794); у Uniswap V3 его нет.

alloy::sol! {
    interface IPoolImmutables {
        function factory() external view returns (address);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function fee() external view returns (uint24);
        function tickSpacing() external view returns (int24);
        function lmPool() external view returns (address);
    }

    interface IInfinityKeys {
        function poolIdToPoolKey(bytes32 id) external view returns (address currency0, address currency1, address hooks, address poolManager, uint24 fee, bytes32 parameters);
    }

    interface IPositionManagerKeys {
        function poolKeys(bytes25 poolId) external view returns (address currency0, address currency1, uint24 fee, int24 tickSpacing, address hooks);
        function poolManager() external view returns (address);
    }
}
