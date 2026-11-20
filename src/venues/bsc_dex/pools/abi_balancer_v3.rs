//! Файл: venues/bsc_dex/pools/abi_balancer_v3.rs
//! Что делает: решение 441 (T433-F2) — ABI Balancer V3 (вид balancer_v3): хранилище Vault V3 — состояние пула
//! (getPoolTokenInfo, getCurrentLiveBalances, getPoolTokenRates, getPoolConfig, getHooksConfig, isPoolPaused), хранилища
//! (isVaultPaused, getMinimumTradeAmount), обмен (unlock, swap, settle, sendTo — шаг BALANCER_V3 исполнителя), событие
//! Swap; пул — getNormalizedWeights / getMinTokenBalances (весовой), getAmplificationParameter и события рампы A
//! (стейблсвап); фабрика — isPoolFromFactory; роутер — querySwapSingleTokenExactIn (квотер сверки).
//! Кто вызывает: pools/verify_balancer_v3.rs, load_balancer_v3*.rs, events_balancer_v3.rs, quote/quoter_balancer_v3.rs,
//! check/probe_exec.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адреса — dex.vault, dex.quoter, dex.bv3_* [DEX]).
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8, build-info v3/tasks/20241204-v3-vault (код —
//! balancer-v3-monorepo@25d73b3d по readme задачи) → @balancer-labs/v3-interfaces/contracts/vault: IVaultMain.sol L27
//! unlock, L49 settle, L60 sendTo, L74 swap; IVaultExtension.sol L180 getPoolTokenRates, L200 getPoolTokenInfo, L219
//! getCurrentLiveBalances, L227 getPoolConfig, L235 getHooksConfig, L294 isPoolPaused; IVaultAdmin.sol L93
//! getMinimumTradeAmount, L115 isVaultPaused; IVaultEvents.sol L53-L61 Swap (pool, tokenIn, tokenOut — indexed);
//! VaultTypes.sol L17-L22 LiquidityManagement, L40-L51 PoolConfig, L73-L85 HooksConfig, L192-L195 TokenType (enum —
//! uint8 в ABI), L227-L231 TokenInfo, L287-L295 VaultSwapParams, L263 SwapKind (EXACT_IN = 0). Пулы: build-info
//! 20260115-v3-weighted-pool-v2 IWeightedPool.sol L60 getNormalizedWeights, L79 getMinTokenBalances;
//! 20241205-v3-stable-pool StablePool.sol L77 / L83 AmpUpdateStarted / AmpUpdateStopped, L261-L264
//! getAmplificationParameter; фабрики — v3-pool-utils BasePoolFactory.sol L69-L72 isPoolFromFactory. Роутер: build-info
//! v3/tasks/20250307-v3-router-v2 → contracts/Router.sol L992-L1021 querySwapSingleTokenExactIn, L1024-L1053 querySwapSingleTokenExactOut (ревью 441, п. 3: сверка
//! обратного расчёта на записи; через Vault.quote —
//! только eth_call без from: tx.origin = 0, EVMCallModeHelpers.isStaticCall, VaultExtension.sol L849-L866).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

alloy::sol! {
    interface IBalancerV3Vault {
        struct TokenInfo {
            uint8 tokenType;
            address rateProvider;
            bool paysYieldFees;
        }
        struct LiquidityManagement {
            bool disableUnbalancedLiquidity;
            bool enableAddLiquidityCustom;
            bool enableRemoveLiquidityCustom;
            bool enableDonation;
        }
        struct PoolConfig {
            LiquidityManagement liquidityManagement;
            uint256 staticSwapFeePercentage;
            uint256 aggregateSwapFeePercentage;
            uint256 aggregateYieldFeePercentage;
            uint40 tokenDecimalDiffs;
            uint32 pauseWindowEndTime;
            bool isPoolRegistered;
            bool isPoolInitialized;
            bool isPoolPaused;
            bool isPoolInRecoveryMode;
        }
        struct HooksConfig {
            bool enableHookAdjustedAmounts;
            bool shouldCallBeforeInitialize;
            bool shouldCallAfterInitialize;
            bool shouldCallComputeDynamicSwapFee;
            bool shouldCallBeforeSwap;
            bool shouldCallAfterSwap;
            bool shouldCallBeforeAddLiquidity;
            bool shouldCallAfterAddLiquidity;
            bool shouldCallBeforeRemoveLiquidity;
            bool shouldCallAfterRemoveLiquidity;
            address hooksContract;
        }
        struct VaultSwapParams {
            uint8 kind;
            address pool;
            address tokenIn;
            address tokenOut;
            uint256 amountGivenRaw;
            uint256 limitRaw;
            bytes userData;
        }
        function getPoolTokenInfo(address pool) external view returns (address[] memory tokens, TokenInfo[] memory tokenInfo, uint256[] memory balancesRaw, uint256[] memory lastBalancesLiveScaled18);
        function getCurrentLiveBalances(address pool) external view returns (uint256[] memory balancesLiveScaled18);
        function getPoolTokenRates(address pool) external view returns (uint256[] memory decimalScalingFactors, uint256[] memory tokenRates);
        function getPoolConfig(address pool) external view returns (PoolConfig memory poolConfig);
        function getHooksConfig(address pool) external view returns (HooksConfig memory hooksConfig);
        function isPoolPaused(address pool) external view returns (bool poolPaused);
        function isVaultPaused() external view returns (bool vaultPaused);
        function getMinimumTradeAmount() external view returns (uint256 minimumTradeAmount);
        function unlock(bytes calldata data) external returns (bytes memory result);
        function swap(VaultSwapParams memory vaultSwapParams) external returns (uint256 amountCalculatedRaw, uint256 amountInRaw, uint256 amountOutRaw);
        function settle(address token, uint256 amountHint) external returns (uint256 credit);
        function sendTo(address token, address to, uint256 amount) external;

        event Swap(address indexed pool, address indexed tokenIn, address indexed tokenOut, uint256 amountIn, uint256 amountOut, uint256 swapFeePercentage, uint256 swapFeeAmount);
    }

    interface IBalancerV3Pool {
        function getNormalizedWeights() external view returns (uint256[] memory);
        function getMinTokenBalances() external view returns (uint256[] memory minTokenBalances);
        function getAmplificationParameter() external view returns (uint256 value, bool isUpdating, uint256 precision);

        event AmpUpdateStarted(uint256 startValue, uint256 endValue, uint256 startTime, uint256 endTime);
        event AmpUpdateStopped(uint256 currentValue);
    }

    interface IBalancerV3Factory {
        function isPoolFromFactory(address pool) external view returns (bool);
    }

    interface IBalancerV3Router {
        function querySwapSingleTokenExactIn(address pool, address tokenIn, address tokenOut, uint256 exactAmountIn, address sender, bytes memory userData) external returns (uint256 amountCalculated);
        function querySwapSingleTokenExactOut(address pool, address tokenIn, address tokenOut, uint256 exactAmountOut, address sender, bytes memory userData) external returns (uint256 amountCalculated);
    }
}

/// topic0 событий самого пула, после которых он перечитывается: рампа A стейблсвапа (StablePool.sol L77, L83).
/// Балансы, комиссию и паузу меняет хранилище своими событиями (лог от адреса Vault), курсы — поставщики курса без
/// событий пула, поэтому вид перечитывается и на каждом блоке (pools/per_block.rs). Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        IBalancerV3Pool::AmpUpdateStarted::SIGNATURE_HASH,
        IBalancerV3Pool::AmpUpdateStopped::SIGNATURE_HASH,
    ]
}
