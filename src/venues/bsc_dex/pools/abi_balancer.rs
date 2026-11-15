//! Файл: venues/bsc_dex/pools/abi_balancer.rs
//! Что делает: ABI Balancer V2 (решение 289, вид balancer_v2; ACSI Finance BSC): хранилище Vault — getPool (адрес и
//! специализация пула по id), getPoolTokens (токены и полные балансы), queryBatchSwap (выход обмена через eth_call —
//! квотер сверки); пул — getVault, getPoolId, getSwapFeePercentage, getPausedState, getNormalizedWeights (весовой),
//! getAmplificationParameter (стейблсвап), события пула (комиссия, пауза, рампа A); фабрики пулов — isPoolFromFactory.
//! Кто вызывает: pools/verify_balancer.rs, load_balancer.rs, events_balancer.rs, quote/quoter_balancer.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адреса — dex.vault, dex.factory, dex.balancer_stable_factory [DEX]).
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 — build-info v2/tasks/20210418-vault (Vault: 45 из 45
//! файлов = Sourcify ACSI 0xa82f327b…ceced, exact_match), v2/deprecated/20210418-weighted-pool и 20210624-stable-pool
//! (пулы и фабрики ACSI — Sourcify exact_match, тексты совпали). Строки (выдержки — tests/data/balancer_acsi_2026-11-15/
//! source_excerpt.txt): IVault.sol L240 getPool, L334-L341 getPoolTokens, L498 SwapKind, L587-L593 BatchSwapStep,
//! L598-L604 событие Swap (выпускает хранилище, id пула — topic1), L623-L628 FundManagement, L645-L650 queryBatchSwap; Swaps.sol L446-L535 (queryBatchSwap откатывает с дельтами и
//! отдаёт их ответом; проверок sender/recipient нет); BasePool.sol L88 SwapFeePercentageChanged, L155-L169 getVault /
//! getPoolId / getSwapFeePercentage; TemporarilyPausable.sol L70-L83 getPausedState (paused — уже с учётом конца
//! буферного периода), ITemporarilyPausable.sol L24 PausedStateChanged; WeightedPool.sol L157 getNormalizedWeights;
//! StablePool.sol L47-L48 AmpUpdateStarted / AmpUpdateStopped, L615-L626 getAmplificationParameter; BasePoolFactory.sol L48-L60
//! isPoolFromFactory.

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

alloy::sol! {
    interface IBalancerVault {
        struct BatchSwapStep {
            bytes32 poolId;
            uint256 assetInIndex;
            uint256 assetOutIndex;
            uint256 amount;
            bytes userData;
        }
        struct FundManagement {
            address sender;
            bool fromInternalBalance;
            address recipient;
            bool toInternalBalance;
        }
        function getPool(bytes32 poolId) external view returns (address, uint8);
        function getPoolTokens(bytes32 poolId) external view returns (address[] memory tokens, uint256[] memory balances, uint256 lastChangeBlock);
        function queryBatchSwap(uint8 kind, BatchSwapStep[] memory swaps, address[] memory assets, FundManagement memory funds) external returns (int256[] memory assetDeltas);

        event Swap(bytes32 indexed poolId, address indexed tokenIn, address indexed tokenOut, uint256 amountIn, uint256 amountOut);
    }

    interface IBalancerPool {
        function getVault() external view returns (address);
        function getPoolId() external view returns (bytes32);
        function getSwapFeePercentage() external view returns (uint256);
        function getPausedState() external view returns (bool paused, uint256 pauseWindowEndTime, uint256 bufferPeriodEndTime);
        function getNormalizedWeights() external view returns (uint256[] memory);
        function getAmplificationParameter() external view returns (uint256 value, bool isUpdating, uint256 precision);

        event SwapFeePercentageChanged(uint256 swapFeePercentage);
        event PausedStateChanged(bool paused);
        event AmpUpdateStarted(uint256 startValue, uint256 endValue, uint256 startTime, uint256 endTime);
        event AmpUpdateStopped(uint256 currentValue);
    }

    interface IBalancerFactory {
        function isPoolFromFactory(address pool) external view returns (bool);
    }

    interface IBalancerToken {
        function decimals() external view returns (uint8);
    }
}

/// topic0 событий самого пула, после которых он перечитывается: комиссия, пауза, рампа A. Балансы меняет хранилище своими
/// событиями (Swap, PoolBalanceChanged, PoolBalanceManaged — лог от адреса Vault, не пула), поэтому вид перечитывается и
/// на каждом блоке (pools/per_block.rs). Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        IBalancerPool::SwapFeePercentageChanged::SIGNATURE_HASH,
        IBalancerPool::PausedStateChanged::SIGNATURE_HASH,
        IBalancerPool::AmpUpdateStarted::SIGNATURE_HASH,
        IBalancerPool::AmpUpdateStopped::SIGNATURE_HASH,
    ]
}
