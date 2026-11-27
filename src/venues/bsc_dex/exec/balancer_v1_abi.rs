//! The generated constructors retain all fields of the original BPool and wallet events.
#![allow(clippy::too_many_arguments)]
alloy::sol! {
    interface IBPoolWallet {
        struct WalletOrder {address pool;address assetIn;address assetOut;uint256 maximum;uint256 minimum;uint256 maxPrice;uint256 deadline;bool exactOutput;}
        function execute(WalletOrder order) external payable returns(uint256,uint256);
        function factory() external view returns(address);
        function wrapped() external view returns(address);
        event Paid(address indexed payer,address indexed recipient,address indexed pool,address assetIn,address assetOut,bool exactOutput,uint256 spent,uint256 received);
        event LOG_SWAP(address indexed caller,address indexed tokenIn,address indexed tokenOut,uint256 tokenAmountIn,uint256 tokenAmountOut);
    }
}
