//! Публичный ABI LambdaClass PropAMMRouter V1 и строгой заявки владельца.
//! https://github.com/lambdaclass/propamm-router-contracts
#![allow(clippy::too_many_arguments)]
alloy::sol! {
    interface IProp {
        function swapViaVenueV1(address venue,address tokenIn,address tokenOut,uint256 amountIn,uint256 minOut,address recipient,uint256 deadline) external returns(uint256,address);
        function quoteVenueV1(address venue,address tokenIn,address tokenOut,uint256 amountIn) external returns(uint256 amountOut,address quotedVenue);
        function isWhitelistedVenue(address venue) external view returns(bool);
        function paused() external view returns(bool);
        function fallbackSwapRouter() external view returns(address);
        event Swapped(address indexed sender,address indexed tokenIn,address indexed tokenOut,uint256 amountIn,uint256 amountOut,address recipient,address marketMaker);
    }
    interface IStrict {
        function market() external view returns(address);
        function vault() external view returns(address);
        function router() external view returns(address);
        function executeWithFallback(address venue,address input,address output,uint256 amount,uint256 minimum,uint256 deadline,address fallbackRouter) external returns(uint256);
        function execute(address venue,address input,address output,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);
        event Filled(address indexed payer,address indexed recipient,address indexed venue,address input,address output,uint256 spent,uint256 received);
    }
}
