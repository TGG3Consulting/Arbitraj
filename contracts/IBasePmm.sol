// SPDX-License-Identifier: MIT
// ABI закреплённых рынков Base и канонического WETH, используемые общим BasePmmExecutor.
pragma solidity 0.8.37;

interface IElfomoRouter {
    function getAmountOut(address from, address to, uint256 amount) external view returns (uint256);
    function swap(address from, address to, int256 amount, uint256 minimum, address receiver, uint256 partner) external;
}

interface IElfomoToken {
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

interface ILunarPool {
    struct Order { address tokenIn; address tokenOut; address recipient; uint256 amountIn; uint256 minimum; uint256 deadline; }
    function isFresh() external view returns (bool);
    function paused() external view returns (bool);
    function quoteExactIn(address tokenIn, address tokenOut, uint256 amountIn) external view returns (uint256);
    function swapExactIn(Order calldata order) external returns (uint256);
    function swapExactInNative(address tokenOut, address recipient, uint256 minimum, uint256 deadline) external payable returns (uint256);
}

interface IBaseWrappedEther {
    function deposit() external payable;
    function withdraw(uint256) external;
    function transfer(address, uint256) external returns (bool);
}

interface IBaiEntry {
    function quoteFor(address base, address tokenIn, uint256 amountIn, address taker)
        external view returns (uint256 amountOut, uint256 fee);
    function swapExactAmountIn(address base, address tokenIn, uint256 amountIn, uint256 minimum, address recipient)
        external returns (uint256 amountOut);
}
