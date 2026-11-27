//! Точный ABI опубликованных событий Integral SIZE и модуля кошелька.
#![allow(clippy::too_many_arguments)]
use alloy::sol;
sol! {interface IRelay {
    struct Order {address input;address output;uint256 source;uint256 destination;bool wrap;address recipient;uint32 deadline;}
    function sell(Order calldata order) external returns(uint256);
    function buy(Order calldata order) external returns(uint256);
    event Buy(address indexed sender,address tokenIn,address tokenOut,uint256 amountIn,uint256 amountInMax,uint256 amountOut,bool wrapUnwrap,uint256 fee,address indexed to,address orderContract,uint256 indexed orderId);
    event Sell(address indexed sender,address tokenIn,address tokenOut,uint256 amountIn,uint256 amountOut,uint256 amountOutMin,bool wrapUnwrap,uint256 fee,address indexed to,address orderContract,uint256 indexed orderId);
}
interface IModule {
    function swapFrom(address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline) external returns(uint256);
    function buy(address input,address output,uint256 maxInput,uint256 targetOut,address recipient,uint256 deadline) external returns(uint256,uint256);
    event Filled(address indexed payer,address indexed recipient,address indexed pair,address input,address output,uint256 spent,uint256 received,uint256 orderId,bool exactOutput);
}}
