//! ABI проверенного роутера ElfomoFi. Событие ElfomoTrade содержит восемь параметров.
#![allow(clippy::too_many_arguments)]
use alloy::sol;

sol! {
    interface IElfomo {
        struct TokenPair { address tokenA; address tokenB; }
        function getSupportedPairs() external view returns (TokenPair[]);
        function getAmountOut(address fromToken,address toToken,uint256 fromAmount) external view returns (uint256);
        function getAmountIn(address fromToken,address toToken,uint256 toAmount) external view returns (uint256);
        event ElfomoTrade(uint256 indexed quoteId,uint256 indexed partnerId,address executor,address receiver,address fromToken,address toToken,uint256 fromAmount,uint256 toAmount);
    }
}
