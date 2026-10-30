//! Файл: venues/bsc_dex/node/abi_dex.rs
//! Что делает: ABI роутеров и квотеров DEX — для пробных обменов (check/*), сверки с квотером (quote/quoter.rs)
//! и прямого обмена (exec/*).
//! Кто вызывает: check/*, quote/quoter.rs, exec/*.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (адреса — dex.router, dex.quoter по виду пула).
//!
//! Сигнатуры из официальных исходников:
//! - V2 Router02: WETH() (обёртка нативной монеты — в BSC WBNB; IUniswapV2Router01.sol#L5, IPancakeRouter01.sol#L5
//!   pancake-swap-periphery d769a6d1), swapExactTokensForTokens(…, deadline), …SupportingFeeOnTransferTokens, getAmountsOut —
//!   https://github.com/Uniswap/v2-periphery/blob/ed24991304291297c3b4a52818d02f46a17aa9a2/contracts/interfaces/IUniswapV2Router02.sol#L24-L30 ,
//!   …/IUniswapV2Router01.sol#L61-L94 (PancakeRouter — те же: research/flow/bsc_dex.md, раздел 5).
//! - PancakeSwap V3 SwapRouter.exactInputSingle (с deadline в структуре) —
//!   https://github.com/pancakeswap/pancake-v3-contracts/blob/986847948755cba528324d41be19480731c36c2a/projects/v3-periphery/contracts/interfaces/ISwapRouter.sol#L10-L24 .
//! - Uniswap SwapRouter02 (V3): exactInputSingle без deadline, deadline — multicall(uint256, bytes[]) —
//!   https://github.com/Uniswap/swap-router-contracts/blob/70bc2e40dfca294c1cea9bf67a4036732ee54303/contracts/interfaces/IV3SwapRouter.sol#L10-L18 ,
//!   …/contracts/interfaces/IMulticallExtended.sol#L15.
//! - QuoterV2.quoteExactInputSingle — pancake-v3-contracts …/v3-periphery/contracts/interfaces/IQuoterV2.sol#L27-L53
//!   (Uniswap QuoterV2 — та же структура).
//! - CLQuoter/BinQuoter (Infinity) и V4Quoter (Uniswap): quoteExactInputSingle((PoolKey, bool, uint128, bytes)) →
//!   (amountOut, gasEstimate) — infinity-periphery src/interfaces/IQuoter.sol#L18-L23, src/pool-cl/interfaces/ICLQuoter.sol#L20-L22;
//!   v4-periphery src/interfaces/IV4Quoter.sol#L13-L36.
//! - KyberSwap Elastic QuoterV2.quoteExactInputSingle((tokenIn, tokenOut, amountIn, feeUnits, limitSqrtP)) → QuoteOutput
//!   (usedAmount, returnedAmount, afterSqrtP, initializedTicksCrossed, gasEstimate) — ks-elastic-sc
//!   contracts/interfaces/periphery/IQuoterV2.sol#L34-L52 (решение 81; своя структура, не V3 QuoterV2).
//! - Algebra (THENA Fusion) QuoterV2.quoteExactInputSingle((tokenIn, tokenOut, amountIn, limitSqrtPrice)) →
//!   (amountOut, sqrtPriceX96After, initializedTicksCrossed, gasEstimate) — @cryptoalgebra/v1.9-periphery
//!   contracts/interfaces/IQuoterV2.sol (решение 81/80, фаза B): БЕЗ параметра fee — комиссия у Algebra динамическая.

// Код функций генерирует alloy::sol! — число аргументов задаёт ABI контракта.
#![allow(clippy::too_many_arguments)]

alloy::sol! {
    interface IV2Router {
        function WETH() external pure returns (address);
        function getAmountsOut(uint256 amountIn, address[] calldata path) external view returns (uint256[] memory amounts);
        function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, address[] calldata path, address to, uint256 deadline) external returns (uint256[] memory amounts);
        function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256 amountIn, uint256 amountOutMin, address[] calldata path, address to, uint256 deadline) external;
    }

    interface IPancakeSwapRouterV3 {
        struct ExactInputSingleParams { address tokenIn; address tokenOut; uint24 fee; address recipient; uint256 deadline; uint256 amountIn; uint256 amountOutMinimum; uint160 sqrtPriceLimitX96; }
        function exactInputSingle(ExactInputSingleParams calldata params) external payable returns (uint256 amountOut);
    }

    interface IUniswapSwapRouter02 {
        struct ExactInputSingleParams { address tokenIn; address tokenOut; uint24 fee; address recipient; uint256 amountIn; uint256 amountOutMinimum; uint160 sqrtPriceLimitX96; }
        function exactInputSingle(ExactInputSingleParams calldata params) external payable returns (uint256 amountOut);
        function multicall(uint256 deadline, bytes[] calldata data) external payable returns (bytes[] memory results);
    }

    interface IQuoterV2 {
        struct QuoteExactInputSingleParams { address tokenIn; address tokenOut; uint256 amountIn; uint24 fee; uint160 sqrtPriceLimitX96; }
        function quoteExactInputSingle(QuoteExactInputSingleParams memory params) external returns (uint256 amountOut, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate);
    }

    interface IInfinityQuoter {
        struct PoolKey { address currency0; address currency1; address hooks; address poolManager; uint24 fee; bytes32 parameters; }
        struct QuoteExactSingleParams { PoolKey poolKey; bool zeroForOne; uint128 exactAmount; bytes hookData; }
        function quoteExactInputSingle(QuoteExactSingleParams memory params) external returns (uint256 amountOut, uint256 gasEstimate);
    }

    interface IV4Quoter {
        struct PoolKey { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
        struct QuoteExactSingleParams { PoolKey poolKey; bool zeroForOne; uint128 exactAmount; bytes hookData; }
        function quoteExactInputSingle(QuoteExactSingleParams memory params) external returns (uint256 amountOut, uint256 gasEstimate);
    }

    interface IElasticQuoterV2 {
        struct QuoteExactInputSingleParams { address tokenIn; address tokenOut; uint256 amountIn; uint24 feeUnits; uint160 limitSqrtP; }
        struct QuoteOutput { uint256 usedAmount; uint256 returnedAmount; uint160 afterSqrtP; uint32 initializedTicksCrossed; uint256 gasEstimate; }
        function quoteExactInputSingle(QuoteExactInputSingleParams memory params) external returns (QuoteOutput memory);
    }

    interface IAlgebraQuoterV2 {
        struct QuoteExactInputSingleParams { address tokenIn; address tokenOut; uint256 amountIn; uint160 limitSqrtPrice; }
        function quoteExactInputSingle(QuoteExactInputSingleParams memory params) external returns (uint256 amountOut, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;

    /// Селекторы совпадают с исследованием (research/flow/bsc_dex.md, раздел 2; bsc_v4.md, раздел 5).
    #[test]
    fn selectors_match_research() {
        let h = |s: [u8; 4]| alloy_primitives::hex::encode(s);
        assert_eq!(
            h(IV2Router::swapExactTokensForTokensCall::SELECTOR),
            "38ed1739"
        );
        assert_eq!(
            h(IV2Router::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::SELECTOR),
            "5c11d795"
        );
        assert_eq!(
            h(IPancakeSwapRouterV3::exactInputSingleCall::SELECTOR),
            "414bf389"
        );
        assert_eq!(
            h(IUniswapSwapRouter02::exactInputSingleCall::SELECTOR),
            "04e45aaf"
        );
        assert_eq!(
            h(IQuoterV2::quoteExactInputSingleCall::SELECTOR),
            "c6a5026a"
        );
        assert_eq!(
            h(IInfinityQuoter::quoteExactInputSingleCall::SELECTOR),
            "9938b8ed"
        );
        assert_eq!(
            h(IV4Quoter::quoteExactInputSingleCall::SELECTOR),
            "aa9d21cb"
        );
        // keccak256("quoteExactInputSingle((address,address,uint256,uint24,uint160))")[..4] — ks-elastic-sc QuoterV2.
        assert_eq!(
            h(IElasticQuoterV2::quoteExactInputSingleCall::SELECTOR),
            alloy_primitives::hex::encode(
                &alloy_primitives::keccak256(
                    "quoteExactInputSingle((address,address,uint256,uint24,uint160))".as_bytes()
                )[..4]
            )
        );
        // keccak256("quoteExactInputSingle((address,address,uint256,uint160))")[..4] — @cryptoalgebra/v1.9-periphery
        // QuoterV2 (без параметра fee: комиссия динамическая).
        assert_eq!(
            h(IAlgebraQuoterV2::quoteExactInputSingleCall::SELECTOR),
            alloy_primitives::hex::encode(
                &alloy_primitives::keccak256(
                    "quoteExactInputSingle((address,address,uint256,uint160))".as_bytes()
                )[..4]
            )
        );
    }
}
