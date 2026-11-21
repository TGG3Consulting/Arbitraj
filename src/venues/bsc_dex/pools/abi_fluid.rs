//! Файл: venues/bsc_dex/pools/abi_fluid.rs
//! Что делает: ABI Fluid DEX T1 (решение 440, вид fluid_t1): пул FluidDexT1 — constantsView / constantsView2 (токены,
//! слой ликвидности, фабрика, множители точности), readFromStorage (слова dexVariables / dexVariables2), VERSION (есть
//! только у пулов 1.1.0+), swapIn (обмен с точным входом), события пула; фабрика FluidDexFactory — isDex; официальный
//! резолвер FluidDexReservesResolver — getPoolReservesAdjusted (центральная цена, резервы частей в приведённых
//! единицах, пределы снятия/займа) и estimateSwapIn (сверка выхода). Слоты: DEX_VARIABLES_SLOT = 0,
//! DEX_VARIABLES2_SLOT = 1. Нативная монета пула — NATIVE_TOKEN 0xEeee… (константа протокола).
//! Кто вызывает: pools/{load_fluid, verify_fluid, events_fluid}.rs, quote/quoter_fluid.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (фабрика — dex.factory, резолвер — dex.quoter [DEX]).
//!
//! Источник (решение 23): github.com/Instadapp/fluid-contracts-public@404d73f91e6806f881f8dabeb30494f93f9e99b7 —
//! contracts/protocols/dex/poolT1/coreModule/core/main.sol L689-L700 (swapIn), L913-L928, L964-L992 (constantsView,
//! constantsView2), L21-L23 (VERSION — «pools deployed before 1.1.0 have no VERSION»); coreModule/structs.sol
//! L122-L152; coreModule/events.sol L10-L75; adminModule/events.sol L9-L132; common/constantVariables.sol L17
//! (NATIVE_TOKEN); libraries/dexSlotsLink.sol L9-L11; protocols/dex/factory/main.sol L253-L261 (isDex);
//! periphery/resolvers/dexReserves/structs.sol L14-L39, main.sol L734-L903 (estimateSwapIn), L1176-L1192
//! (getPoolReservesAdjusted). Развёрнутые в Arbitrum One адреса и ABI — deployments/arbitrum/*.json того же коммита;
//! селекторы swapIn 0x2668dfaa, constantsView 0xb7791bf2, constantsView2 0x1595cbd3, readFromStorage 0xb5c736e4,
//! isDex 0x3c11e12a, getPoolReservesAdjusted 0xbd964d38, estimateSwapIn 0xbb39e3a1 — PUSH4 есть в deployedBytecode
//! Dex_USDC_ETH.json / DexFactory.json / DexReservesResolver.json; VERSION 0xffa1ad74 у Dex_USDC_ETH — нет.

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};

/// NATIVE_TOKEN пула (constantVariables.sol L17) — константа протокола: метка нативной монеты, не ERC-20.
pub const NATIVE: Address =
    alloy_primitives::address!("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");

/// Слот dexVariables (dexSlotsLink.sol L9): цены и время последнего обмена.
pub const SLOT_VARS: u8 = 0;
/// Слот dexVariables2 (dexSlotsLink.sol L11): части, комиссия, доля протокола, пауза.
pub const SLOT_VARS2: u8 = 1;

alloy::sol! {
    interface IFluidDexT1 {
        struct Implementations {
            address shift;
            address admin;
            address colOperations;
            address debtOperations;
            address perfectOperationsAndSwapOut;
        }
        struct ConstantViews {
            uint256 dexId;
            address liquidity;
            address factory;
            Implementations implementations;
            address deployerContract;
            address token0;
            address token1;
            bytes32 supplyToken0Slot;
            bytes32 borrowToken0Slot;
            bytes32 supplyToken1Slot;
            bytes32 borrowToken1Slot;
            bytes32 exchangePriceToken0Slot;
            bytes32 exchangePriceToken1Slot;
            uint256 oracleMapping;
        }
        struct ConstantViews2 {
            uint256 token0NumeratorPrecision;
            uint256 token0DenominatorPrecision;
            uint256 token1NumeratorPrecision;
            uint256 token1DenominatorPrecision;
        }

        function constantsView() external view returns (ConstantViews memory constantsView_);
        function constantsView2() external view returns (ConstantViews2 memory constantsView2_);
        function readFromStorage(bytes32 slot_) external view returns (uint256 result_);
        function VERSION() external view returns (string memory);
        function swapIn(bool swap0to1_, uint256 amountIn_, uint256 amountOutMin_, address to_) external payable returns (uint256 amountOut_);

        event Swap(bool swap0to1, uint256 amountIn, uint256 amountOut, address to);
        event LogDepositPerfectColLiquidity(uint256 shares, uint256 token0Amt, uint256 token1Amt);
        event LogWithdrawPerfectColLiquidity(uint256 shares, uint256 token0Amt, uint256 token1Amt);
        event LogBorrowPerfectDebtLiquidity(uint256 shares, uint256 token0Amt, uint256 token1Amt);
        event LogPaybackPerfectDebtLiquidity(uint256 shares, uint256 token0Amt, uint256 token1Amt);
        event LogDepositColLiquidity(uint256 amount0, uint256 amount1, uint256 shares);
        event LogWithdrawColLiquidity(uint256 amount0, uint256 amount1, uint256 shares);
        event LogBorrowDebtLiquidity(uint256 amount0, uint256 amount1, uint256 shares);
        event LogPaybackDebtLiquidity(uint256 amount0, uint256 amount1, uint256 shares);
        event LogWithdrawColInOneToken(uint256 shares, uint256 token0Amt, uint256 token1Amt);
        event LogPaybackDebtInOneToken(uint256 shares, uint256 token0Amt, uint256 token1Amt);
        event LogArbitrage(int256 routing, uint256 amtOut);
        event LogTurnOnSmartCol(uint256 token0Amt);
        event LogTurnOnSmartDebt(uint256 token0Amt);
        event LogUpdateFeeAndRevenueCut(uint256 fee, uint256 revenueCut);
        event LogUpdateRangePercents(uint256 upperPercent, uint256 lowerPercent, uint256 shiftTime);
        event LogUpdateThresholdPercent(uint256 upperThresholdPercent, uint256 lowerThresholdPercent, uint256 thresholdShiftTime, uint256 shiftTime);
        event LogUpdateCenterPriceAddress(uint256 centerPriceAddress, uint256 percent, uint256 time);
        event LogUpdateCenterPriceLimits(uint256 maxCenterPrice, uint256 minCenterPrice);
        event LogUpdateUtilizationLimit(uint256 token0UtilizationLimit, uint256 token1UtilizationLimit);
        event LogPauseSwapAndArbitrage();
        event LogUnpauseSwapAndArbitrage();
    }

    interface IFluidDexFactory {
        function isDex(address dex_) external view returns (bool);
    }

    interface IFluidDexReservesResolver {
        struct CollateralReserves {
            uint256 token0RealReserves;
            uint256 token1RealReserves;
            uint256 token0ImaginaryReserves;
            uint256 token1ImaginaryReserves;
        }
        struct DebtReserves {
            uint256 token0Debt;
            uint256 token1Debt;
            uint256 token0RealReserves;
            uint256 token1RealReserves;
            uint256 token0ImaginaryReserves;
            uint256 token1ImaginaryReserves;
        }
        struct TokenLimit {
            uint256 available;
            uint256 expandsTo;
            uint256 expandDuration;
        }
        struct DexLimits {
            TokenLimit withdrawableToken0;
            TokenLimit withdrawableToken1;
            TokenLimit borrowableToken0;
            TokenLimit borrowableToken1;
        }
        struct PoolWithReserves {
            address pool;
            address token0;
            address token1;
            uint256 fee;
            uint256 centerPrice;
            CollateralReserves collateralReserves;
            DebtReserves debtReserves;
            DexLimits limits;
        }

        function getPoolReservesAdjusted(address pool_) external returns (PoolWithReserves memory poolReserves_);
        function estimateSwapIn(address dex_, bool swap0to1_, uint256 amountIn_, uint256 amountOutMin_) external payable returns (uint256 amountOut_);
    }
}

/// topic0 событий изменения ликвидности пула (внесение / снятие / заём / погашение — coreModule/events.sol L16-L70).
/// Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    use IFluidDexT1 as P;
    vec![
        P::LogDepositPerfectColLiquidity::SIGNATURE_HASH,
        P::LogWithdrawPerfectColLiquidity::SIGNATURE_HASH,
        P::LogBorrowPerfectDebtLiquidity::SIGNATURE_HASH,
        P::LogPaybackPerfectDebtLiquidity::SIGNATURE_HASH,
        P::LogDepositColLiquidity::SIGNATURE_HASH,
        P::LogWithdrawColLiquidity::SIGNATURE_HASH,
        P::LogBorrowDebtLiquidity::SIGNATURE_HASH,
        P::LogPaybackDebtLiquidity::SIGNATURE_HASH,
        P::LogWithdrawColInOneToken::SIGNATURE_HASH,
        P::LogPaybackDebtInOneToken::SIGNATURE_HASH,
    ]
}

/// topic0 всех событий пула, после которых он перечитывается (обмен, внутренний арбитраж, ликвидность, настройки
/// пула — adminModule/events.sol). Ошибок нет.
pub fn topics() -> Vec<B256> {
    use IFluidDexT1 as P;
    let mut out = vec![
        P::Swap::SIGNATURE_HASH,
        P::LogArbitrage::SIGNATURE_HASH,
        P::LogTurnOnSmartCol::SIGNATURE_HASH,
        P::LogTurnOnSmartDebt::SIGNATURE_HASH,
        P::LogUpdateFeeAndRevenueCut::SIGNATURE_HASH,
        P::LogUpdateRangePercents::SIGNATURE_HASH,
        P::LogUpdateThresholdPercent::SIGNATURE_HASH,
        P::LogUpdateCenterPriceAddress::SIGNATURE_HASH,
        P::LogUpdateCenterPriceLimits::SIGNATURE_HASH,
        P::LogUpdateUtilizationLimit::SIGNATURE_HASH,
        P::LogPauseSwapAndArbitrage::SIGNATURE_HASH,
        P::LogUnpauseSwapAndArbitrage::SIGNATURE_HASH,
    ];
    out.extend(liquidity_topics());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;

    /// Селекторы — keccak256 подписей исходника; PUSH4 этих селекторов есть в deployedBytecode развёрнутых в Arbitrum
    /// контрактов (deployments/arbitrum, см. шапку).
    #[test]
    fn selectors_match_deployed() {
        let s = IFluidDexT1::swapInCall::SELECTOR;
        assert_eq!(s, [0x26, 0x68, 0xdf, 0xaa]);
        let s = IFluidDexT1::constantsViewCall::SELECTOR;
        assert_eq!(s, [0xb7, 0x79, 0x1b, 0xf2]);
        let s = IFluidDexT1::constantsView2Call::SELECTOR;
        assert_eq!(s, [0x15, 0x95, 0xcb, 0xd3]);
        let s = IFluidDexT1::readFromStorageCall::SELECTOR;
        assert_eq!(s, [0xb5, 0xc7, 0x36, 0xe4]);
        let s = IFluidDexFactory::isDexCall::SELECTOR;
        assert_eq!(s, [0x3c, 0x11, 0xe1, 0x2a]);
        let s = IFluidDexReservesResolver::getPoolReservesAdjustedCall::SELECTOR;
        assert_eq!(s, [0xbd, 0x96, 0x4d, 0x38]);
        let s = IFluidDexReservesResolver::estimateSwapInCall::SELECTOR;
        assert_eq!(s, [0xbb, 0x39, 0xe3, 0xa1]);
        assert_eq!(topics().len(), 22);
    }
}
