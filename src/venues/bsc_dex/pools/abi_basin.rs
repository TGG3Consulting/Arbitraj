//! Файл: venues/bsc_dex/pools/abi_basin.rs
//! Что делает: ABI Basin (решение 485, вид basin_well): скважина Well — tokens, wellFunction, aquifer, getReserves,
//! totalSupply, getSwapOut (сверка выхода), swapFrom (шаг BASIN исполнителя), getImplementation (только у
//! WellUpgradeable за прокси ERC-1967), события скважины (Swap, Shift, Sync, AddLiquidity, RemoveLiquidity,
//! RemoveLiquidityOneToken — все поля не индексированы) и Upgraded прокси; фабрика Aquifer — wellImplementation,
//! событие BoreWell; функции скважины — calcLpTokenSupply (сверка A Stable2 при проверке); таблица Stable2 —
//! getAParameter.
//! Кто вызывает: pools/{load_basin, verify_basin, events_basin, created_log}.rs, quote/quoter_basin.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (Aquifer — dex.factory, списки — dex.basin_* [DEX]).
//!
//! Источник (официальный): BeanstalkFarms/Basin@ecf69233 — src/interfaces/IWell.sol L10-L13 (Call), L31-L77
//! (события), src/Well.sol L106-L158 (tokens, wellFunction, aquifer), L207-L279 (swapFrom, getSwapOut), L694-L696
//! (getReserves), src/WellUpgradeable.sol L132-L134 (getImplementation), src/interfaces/IAquifer.sol L42-L44
//! (BoreWell), src/Aquifer.sol L22 (wellImplementation), src/interfaces/IWellFunction.sol (calcLpTokenSupply),
//! src/interfaces/ILookupTable.sol (getAParameter). Upgraded(address indexed) — OpenZeppelin ERC1967Upgrade
//! (прокси скважин Beanstalk на Arbitrum — ERC1967Proxy: BeanstalkFarms/Beanstalk@6db8c07c
//! protocol/contracts/beanstalk/init/reseed/L2/ReseedBean.sol L147, L225-L257).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

alloy::sol! {
    interface IBasinWell {
        struct Call {
            address target;
            bytes data;
        }
        function tokens() external view returns (address[] memory);
        function wellFunction() external view returns (Call memory);
        function aquifer() external view returns (address);
        function getReserves() external view returns (uint256[] memory reserves);
        function totalSupply() external view returns (uint256);
        function getSwapOut(address fromToken, address toToken, uint256 amountIn)
            external view returns (uint256 amountOut);
        function swapFrom(
            address fromToken,
            address toToken,
            uint256 amountIn,
            uint256 minAmountOut,
            address recipient,
            uint256 deadline
        ) external returns (uint256 amountOut);
        function getImplementation() external view returns (address);

        event Swap(
            address fromToken,
            address toToken,
            uint256 amountIn,
            uint256 amountOut,
            address recipient
        );
        event AddLiquidity(uint256[] tokenAmountsIn, uint256 lpAmountOut, address recipient);
        event RemoveLiquidity(uint256 lpAmountIn, uint256[] tokenAmountsOut, address recipient);
        event RemoveLiquidityOneToken(
            uint256 lpAmountIn,
            address tokenOut,
            uint256 tokenAmountOut,
            address recipient
        );
        event Shift(uint256[] reserves, address toToken, uint256 amountOut, address recipient);
        event Sync(uint256[] reserves, uint256 lpAmountOut, address recipient);
        event Upgraded(address indexed implementation);
    }

    interface IBasinAquifer {
        struct Call {
            address target;
            bytes data;
        }
        function wellImplementation(address well) external view returns (address);
        event BoreWell(
            address well,
            address implementation,
            address[] tokens,
            Call wellFunction,
            Call[] pumps,
            bytes wellData
        );
    }

    interface IBasinWellFunction {
        function calcLpTokenSupply(uint256[] memory reserves, bytes memory data)
            external view returns (uint256 lpTokenSupply);
    }

    interface IBasinLookupTable {
        function getAParameter() external view returns (uint256);
    }
}

/// topic0 событий скважины, меняющих резервы или число LP-токенов, и Upgraded прокси (abi выше). Ошибок нет.
pub fn topics() -> Vec<B256> {
    use IBasinWell as W;
    vec![
        W::Swap::SIGNATURE_HASH,
        W::Shift::SIGNATURE_HASH,
        W::Sync::SIGNATURE_HASH,
        W::Upgraded::SIGNATURE_HASH,
        W::AddLiquidity::SIGNATURE_HASH,
        W::RemoveLiquidity::SIGNATURE_HASH,
        W::RemoveLiquidityOneToken::SIGNATURE_HASH,
    ]
}

/// topic0 событий изменения ликвидности скважины (внесение, снятие, Sync — выпуск LP по излишку балансов). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    use IBasinWell as W;
    vec![
        W::AddLiquidity::SIGNATURE_HASH,
        W::RemoveLiquidity::SIGNATURE_HASH,
        W::RemoveLiquidityOneToken::SIGNATURE_HASH,
        W::Sync::SIGNATURE_HASH,
    ]
}
