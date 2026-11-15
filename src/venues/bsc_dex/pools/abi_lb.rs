//! Файл: venues/bsc_dex/pools/abi_lb.rs
//! Что делает: ABI Liquidity Book V2.1 (решение 290, вид lb_v21; LFJ / Trader Joe на BSC): пул LBPair — поля (фабрика,
//! токены X/Y, шаг бина), состояние (активный бин, параметры комиссии, резервы бинов, следующий непустой бин), квотер
//! сверки getSwapOut и события; фабрика LBFactory — реестр getLBPairInformation (пул по паре и шагу).
//! Кто вызывает: pools/verify_lb.rs, load_lb.rs, load_lb_bins.rs, per_block_lb.rs, events_lb.rs, quote/quoter_lb.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адрес фабрики — dex.factory [DEX]).
//!
//! Источник (решение 23): lfj-gg/joe-v2@v2.1.0 (8eeb42d7) = Sourcify exact_match фабрики BSC 0x8e42…fd87d3efd4e и
//! реализаций пула 0xee5a…86df / 0x3e30…2d8f (tests/data/lb_v21_2026-11-15/README.md). Строки: src/LBPair.sol L124-L126
//! getFactory, L132-L134 getTokenX, L140-L142 getTokenY, L150-L152 getBinStep, L171-L173 getActiveId, L181-L183 getBin,
//! L193-L195 getNextNonEmptyBin, L216-L239 getStaticFeeParameters, L248-L260 getVariableFeeParameters, L424-L468
//! getSwapOut; src/interfaces/ILBPair.sol L37-L78 события; src/LBFactory.sol L170-L177, L607-L614
//! getLBPairInformation (токены сортируются — порядок аргументов не важен), src/interfaces/ILBFactory.sol L42-L47
//! LBPairInformation.

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

alloy::sol! {
    interface ILBPair {
        function getFactory() external view returns (address factory);
        function getTokenX() external view returns (address tokenX);
        function getTokenY() external view returns (address tokenY);
        function getBinStep() external view returns (uint16);
        function getActiveId() external view returns (uint24 activeId);
        function getBin(uint24 id) external view returns (uint128 binReserveX, uint128 binReserveY);
        function getNextNonEmptyBin(bool swapForY, uint24 id) external view returns (uint24 nextId);
        function getStaticFeeParameters() external view returns (uint16 baseFactor, uint16 filterPeriod, uint16 decayPeriod, uint16 reductionFactor, uint24 variableFeeControl, uint16 protocolShare, uint24 maxVolatilityAccumulator);
        function getVariableFeeParameters() external view returns (uint24 volatilityAccumulator, uint24 volatilityReference, uint24 idReference, uint40 timeOfLastUpdate);
        function getSwapOut(uint128 amountIn, bool swapForY) external view returns (uint128 amountInLeft, uint128 amountOut, uint128 fee);

        event DepositedToBins(address indexed sender, address indexed to, uint256[] ids, bytes32[] amounts);
        event WithdrawnFromBins(address indexed sender, address indexed to, uint256[] ids, bytes32[] amounts);
        event CompositionFees(address indexed sender, uint24 id, bytes32 totalFees, bytes32 protocolFees);
        event CollectedProtocolFees(address indexed feeRecipient, bytes32 protocolFees);
        event Swap(address indexed sender, address indexed to, uint24 id, bytes32 amountsIn, bytes32 amountsOut, uint24 volatilityAccumulator, bytes32 totalFees, bytes32 protocolFees);
        event StaticFeeParametersSet(address indexed sender, uint16 baseFactor, uint16 filterPeriod, uint16 decayPeriod, uint16 reductionFactor, uint24 variableFeeControl, uint16 protocolShare, uint24 maxVolatilityAccumulator);
        event FlashLoan(address indexed sender, address indexed receiver, uint24 activeId, bytes32 amounts, bytes32 totalFees, bytes32 protocolFees);
        event ForcedDecay(address indexed sender, uint24 idReference, uint24 volatilityReference);
    }

    interface ILBFactory {
        struct LBPairInformation {
            uint16 binStep;
            address LBPair;
            bool createdByOwner;
            bool ignoredForRouting;
        }
        function getLBPairInformation(address tokenA, address tokenB, uint256 binStep) external view returns (LBPairInformation memory);
    }
}

/// topic0 событий пула, меняющих резервы бинов, активный бин или параметры комиссии (ILBPair.sol L37-L78):
/// DepositedToBins, WithdrawnFromBins, CompositionFees, CollectedProtocolFees, Swap, StaticFeeParametersSet, FlashLoan,
/// ForcedDecay. Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        ILBPair::DepositedToBins::SIGNATURE_HASH,
        ILBPair::WithdrawnFromBins::SIGNATURE_HASH,
        ILBPair::CompositionFees::SIGNATURE_HASH,
        ILBPair::CollectedProtocolFees::SIGNATURE_HASH,
        ILBPair::Swap::SIGNATURE_HASH,
        ILBPair::StaticFeeParametersSet::SIGNATURE_HASH,
        ILBPair::FlashLoan::SIGNATURE_HASH,
        ILBPair::ForcedDecay::SIGNATURE_HASH,
    ]
}

/// topic0 событий изменения ликвидности (DepositedToBins, WithdrawnFromBins). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![
        ILBPair::DepositedToBins::SIGNATURE_HASH,
        ILBPair::WithdrawnFromBins::SIGNATURE_HASH,
    ]
}
