//! Файл: venues/bsc_dex/pools/abi_balancer_v3_kinds.rs
//! Что делает: решение 452 — ABI видов пулов Balancer V3 сверх весового и стейблсвапа (abi_balancer_v3.rs): Gyro 2-CLP
//! и E-CLP (неизменяемые параметры), ReClamm (виртуальные балансы), LBP всех выпусков и LBP с фиксированной ценой
//! (окно, токены, виртуальный резерв, цена), QuantAMM (веса и предел доли сделки); крючки StableSurge и MEV Capture
//! (параметры пула); прекомпайлы Arbitrum ArbSys (версия ArbOS) и ArbOwnerPublic (собираются ли чаевые); событие
//! PoolRegistered хранилища (новый пул без индексатора).
//! Кто вызывает: pools/load_balancer_v3_fixed.rs, load_balancer_v3_kinds.rs, events_balancer_v3.rs, тесты.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адреса — dex.vault, dex.bv3_*, dex.arb_sys, dex.arb_owner_public [DEX]).
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 build-info — @balancer-labs/v3-interfaces: pool-gyro/
//! IGyro2CLPPool.sol (Gyro2CLPPoolImmutableData), IGyroECLPPool.sol L105-L144, L163 (getECLPParams; у 20250124 —
//! функция пула GyroECLPPool.sol L211); reclamm@17262629 contracts/interfaces/IReClammPool.sol (computeCurrentVirtualBalances,
//! getLastVirtualBalances, getCenterednessMargin — во всех трёх выпусках); pool-weighted/ILBPool.sol (20250307 —
//! LBPoolImmutableData L52-L62, 20250701 — L52-L67, isSwapEnabled), ILBPCommon.sol (20251219, 20260501: getTokenIndices,
//! isProjectTokenSwapInBlocked, isSwapEnabled), ILBPool.sol getReserveTokenVirtualBalance; FixedPriceLBPool.sol L87
//! getProjectTokenRate; pool-hooks/IStableSurgeHook.sol, IMevCaptureHook.sol; vault/IVaultEvents.sol L26-L35
//! PoolRegistered, VaultTypes.sol L119-L123 PoolRoleAccounts, L211-L216 TokenConfig. QuantAMM —
//! QuantAMMProtocol/deployment@e7867165 v3/tasks/20250429-v3-arbitrum-quantamm build-info → IQuantAMMWeightedPool.sol
//! L54-L64, L29-L40 и L137 (getQuantAMMWeightedPoolDynamicData — времена интерполяции весов). Прекомпайлы — OffchainLabs/nitro-precompile-interfaces@72f30751 ArbSys.sol L38 (arbOSVersion = 55 + версия
//! ArbOS), ArbOwnerPublic.sol L92 (getCollectTips — с ArbOS 60).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    interface IGyro2Clp {
        struct Gyro2CLPPoolImmutableData {
            address[] tokens;
            uint256[] decimalScalingFactors;
            uint256 sqrtAlpha;
            uint256 sqrtBeta;
        }
        function getGyro2CLPPoolImmutableData() external view returns (Gyro2CLPPoolImmutableData memory data);
    }

    interface IGyroEclp {
        struct EclpParams {
            int256 alpha;
            int256 beta;
            int256 c;
            int256 s;
            int256 lambda;
        }
        struct Vector2 {
            int256 x;
            int256 y;
        }
        struct DerivedEclpParams {
            Vector2 tauAlpha;
            Vector2 tauBeta;
            int256 u;
            int256 v;
            int256 w;
            int256 z;
            int256 dSq;
        }
        function getECLPParams() external view returns (EclpParams memory params, DerivedEclpParams memory d);
    }

    interface IReClamm {
        function computeCurrentVirtualBalances() external view returns (uint256 currentVirtualBalanceA, uint256 currentVirtualBalanceB, bool changed);
        function getLastVirtualBalances() external view returns (uint256 lastVirtualBalanceA, uint256 lastVirtualBalanceB);
        function getCenterednessMargin() external view returns (uint256 centerednessMargin);
    }

    interface ILbpV1 {
        struct LBPoolImmutableData {
            address[] tokens;
            uint256[] decimalScalingFactors;
            uint256[] startWeights;
            uint256[] endWeights;
            uint256 startTime;
            uint256 endTime;
            uint256 projectTokenIndex;
            uint256 reserveTokenIndex;
            bool isProjectTokenSwapInBlocked;
        }
        function getLBPoolImmutableData() external view returns (LBPoolImmutableData memory data);
    }

    interface ILbpV2 {
        struct LBPoolImmutableData {
            address[] tokens;
            uint256[] decimalScalingFactors;
            uint256[] startWeights;
            uint256[] endWeights;
            uint256 startTime;
            uint256 endTime;
            uint256 projectTokenIndex;
            uint256 reserveTokenIndex;
            bool isProjectTokenSwapInBlocked;
            address migrationRouter;
            uint256 bptLockDuration;
            uint256 bptPercentageToMigrate;
            uint256 migrationWeightProjectToken;
            uint256 migrationWeightReserveToken;
        }
        function getLBPoolImmutableData() external view returns (LBPoolImmutableData memory data);
    }

    interface ILbp {
        function isSwapEnabled() external view returns (bool);
        function getTokenIndices() external view returns (uint256 projectTokenIndex, uint256 reserveTokenIndex);
        function isProjectTokenSwapInBlocked() external view returns (bool);
        function getReserveTokenVirtualBalance() external view returns (uint256 raw, uint256 scaled18);
        function getProjectTokenRate() external view returns (uint256);
    }

    interface IQuantAmm {
        struct QuantAMMWeightedPoolImmutableData {
            address[] tokens;
            uint256 oracleStalenessThreshold;
            uint256 poolRegistry;
            int256[][] ruleParameters;
            uint64[] lambda;
            uint64 epsilonMax;
            uint64 absoluteWeightGuardRail;
            uint64 updateInterval;
            uint256 maxTradeSizeRatio;
        }
        function getQuantAMMWeightedPoolImmutableData() external view returns (QuantAMMWeightedPoolImmutableData memory data);
        struct QuantAMMWeightedPoolDynamicData {
            uint256[] balancesLiveScaled18;
            uint256[] tokenRates;
            uint256 totalSupply;
            bool isPoolInitialized;
            bool isPoolPaused;
            bool isPoolInRecoveryMode;
            int256[] firstFourWeightsAndMultipliers;
            int256[] secondFourWeightsAndMultipliers;
            uint40 lastUpdateTime;
            uint40 lastInteropTime;
        }
        function getQuantAMMWeightedPoolDynamicData() external view returns (QuantAMMWeightedPoolDynamicData memory data);
    }

    interface IStableSurgeHook {
        function getMaxSurgeFeePercentage(address pool) external view returns (uint256);
        function getSurgeThresholdPercentage(address pool) external view returns (uint256);
    }

    interface IMevCaptureHook {
        function isMevTaxEnabled() external view returns (bool);
        function getMaxMevSwapFeePercentage() external view returns (uint256);
        function getPoolMevTaxMultiplier(address pool) external view returns (uint256);
        function getPoolMevTaxThreshold(address pool) external view returns (uint256);
    }

    interface IArbSys {
        function arbOSVersion() external view returns (uint256);
    }

    interface IArbOwnerPublic {
        function getCollectTips() external view returns (bool);
    }

    interface IBalancerV3VaultEvents {
        struct TokenConfig {
            address token;
            uint8 tokenType;
            address rateProvider;
            bool paysYieldFees;
        }
        struct PoolRoleAccounts {
            address pauseManager;
            address swapFeeManager;
            address poolCreator;
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
        struct LiquidityManagement {
            bool disableUnbalancedLiquidity;
            bool enableAddLiquidityCustom;
            bool enableRemoveLiquidityCustom;
            bool enableDonation;
        }
        event PoolRegistered(address indexed pool, address indexed factory, TokenConfig[] tokenConfig, uint256 swapFeePercentage, uint32 pauseWindowEndTime, PoolRoleAccounts roleAccounts, HooksConfig hooksConfig, LiquidityManagement liquidityManagement);
    }
}
