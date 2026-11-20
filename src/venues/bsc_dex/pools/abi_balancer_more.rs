//! Файл: venues/bsc_dex/pools/abi_balancer_more.rs
//! Что делает: решение 473 (T450-03…07) — ABI пулов Balancer V2 других фабрик: composable stable (getScalingFactors,
//! getBptIndex, getRateProviders, getTokenRateCache), meta-stable (getRateProviders, getPriceRateCache), stable phantom
//! (getScalingFactor, getBptIndex), LBP / investment / managed (getSwapEnabled, getCollectedManagementFees,
//! getActualSupply, getCircuitBreakerState), linear (getMainIndex, getWrappedIndex, getBptIndex, getWrappedTokenRate,
//! getTargets, totalSupply), поставщик курса (getRate) и topic0 их событий, после которых пул перечитывается.
//! Кто вызывает: pools/load_balancer_more*.rs, abi_balancer.rs (topics).
//! От чего зависит: alloy::sol!.
//! Настройки: нет.
//!
//! Источник (решение 23) — build-info balancer-deployments@b55dc2b8: ComposableStablePoolRates.sol (20240223-v6) L49-L50
//! события, L147-L165 getTokenRateCache, ComposableStablePoolStorage.sol L184 getBptIndex, L289-L298 getRateProviders,
//! BasePool.sol L672 getScalingFactors; MetaStablePool.sol (20210727) L448-L471 getRateProviders / getPriceRateCache,
//! события PriceRateCacheUpdated / PriceRateProviderSet; StablePhantomPool.sol (20211208) L172 getBptIndex, L615
//! getScalingFactor, события TokenRateCacheUpdated(IERC20,uint256) / TokenRateProviderSet(IERC20,…);
//! LiquidityBootstrappingPool.sol (20210721) L76-L82 события, L137 getSwapEnabled; InvestmentPool.sol (20210907)
//! L157 getSwapEnabled, L237-L249 getCollectedManagementFees (уже downscaleDown); IManagedPool.sol (20221021,
//! 20230411) события L21-L47, ManagedPoolSettings.sol (20221021) L177 getActualSupply, L382 getSwapEnabled, L743-L774
//! getCircuitBreakerState (границы × множитель токена); LinearPool.sol (20211208) L158-L168 индексы, L569
//! getWrappedTokenRate, L575 getTargets, L99 TargetsSet; IRateProvider.sol L22 getRate.

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

alloy::sol! {
    interface IComposable {
        function getScalingFactors() external view returns (uint256[] memory);
        function getBptIndex() external view returns (uint256);
        function getRateProviders() external view returns (address[] memory);
        function getTokenRateCache(address token) external view returns (uint256 rate, uint256 oldRate, uint256 duration, uint256 expires);

        event TokenRateCacheUpdated(uint256 indexed tokenIndex, uint256 rate);
        event TokenRateProviderSet(uint256 indexed tokenIndex, address indexed provider, uint256 cacheDuration);
    }

    interface IMetaStable {
        function getRateProviders() external view returns (address[] memory providers);
        function getPriceRateCache(address token) external view returns (uint256 rate, uint256 duration, uint256 expires);

        event PriceRateCacheUpdated(address indexed token, uint256 rate);
        event PriceRateProviderSet(address indexed token, address indexed provider, uint256 cacheDuration);
    }

    interface IPhantom {
        function getScalingFactor(address token) external view returns (uint256);

        event TokenRateCacheUpdated(address indexed token, uint256 rate);
        event TokenRateProviderSet(address indexed token, address indexed provider, uint256 cacheDuration);
    }

    interface ISmartWeighted {
        function getSwapEnabled() external view returns (bool);
        function getCollectedManagementFees() external view returns (address[] memory tokens, uint256[] memory collectedFees);
        function getActualSupply() external view returns (uint256);
        function getCircuitBreakerState(address token) external view returns (uint256 bptPrice, uint256 referenceWeight, uint256 lowerBound, uint256 upperBound, uint256 lowerBptPriceBound, uint256 upperBptPriceBound);

        event GradualWeightUpdateScheduled(uint256 startTime, uint256 endTime, uint256[] startWeights, uint256[] endWeights);
        event SwapEnabledSet(bool swapEnabled);
        event GradualSwapFeeUpdateScheduled(uint256 startTime, uint256 endTime, uint256 startSwapFeePercentage, uint256 endSwapFeePercentage);
        event CircuitBreakerSet(address indexed token, uint256 bptPrice, uint256 lowerBoundPercentage, uint256 upperBoundPercentage);
        event ManagementAumFeeCollected(uint256 bptAmount);
        event TokenAdded(address indexed token, uint256 normalizedWeight);
        event TokenRemoved(address indexed token);
    }

    interface ILinear {
        function getMainIndex() external view returns (uint256);
        function getWrappedIndex() external view returns (uint256);
        function getWrappedTokenRate() external view returns (uint256);
        function getTargets() external view returns (uint256 lowerTarget, uint256 upperTarget);
        function totalSupply() external view returns (uint256);

        event TargetsSet(address indexed token, uint256 lowerTarget, uint256 upperTarget);
    }

    interface IRateProvider {
        function getRate() external view returns (uint256);
    }
}

/// topic0 событий пулов решения 473, после которых пул перечитывается (балансы и время меняют курсы и веса и без
/// событий — пул и так перечитывается каждым блоком, pools/per_block_balancer.rs). Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        IComposable::TokenRateCacheUpdated::SIGNATURE_HASH,
        IComposable::TokenRateProviderSet::SIGNATURE_HASH,
        IMetaStable::PriceRateCacheUpdated::SIGNATURE_HASH,
        IMetaStable::PriceRateProviderSet::SIGNATURE_HASH,
        IPhantom::TokenRateCacheUpdated::SIGNATURE_HASH,
        IPhantom::TokenRateProviderSet::SIGNATURE_HASH,
        ISmartWeighted::GradualWeightUpdateScheduled::SIGNATURE_HASH,
        ISmartWeighted::SwapEnabledSet::SIGNATURE_HASH,
        ISmartWeighted::GradualSwapFeeUpdateScheduled::SIGNATURE_HASH,
        ISmartWeighted::CircuitBreakerSet::SIGNATURE_HASH,
        ISmartWeighted::ManagementAumFeeCollected::SIGNATURE_HASH,
        ISmartWeighted::TokenAdded::SIGNATURE_HASH,
        ISmartWeighted::TokenRemoved::SIGNATURE_HASH,
        ILinear::TargetsSet::SIGNATURE_HASH,
    ]
}
