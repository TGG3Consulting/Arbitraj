// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

/// @dev Тестовая покупка USDC из собственных ETH и проверка кассы в одном каноническом eth_call.
interface IKCashToken {
    function approve(address spender, uint256 value) external returns (bool);
    function balanceOf(address who) external view returns (uint256);
}
interface IKCashSwap {
    function swap(bytes32 pair, address src, address dst, uint256 amount, uint256 minimum, uint256 deadline) external returns (uint256);
}
interface IKCashFunding {
    struct ExactOut { address tokenIn; address tokenOut; uint24 fee; address recipient; uint256 out; uint256 cap; uint160 limit; }
    function exactOutputSingle(ExactOut calldata order) external payable returns(uint256);
}
contract KaliberCashProbe {
    struct Order { address funding; address weth; address stable; address target; address cash; bytes32 pair; uint256 quantity; uint256 minimum; }
    constructor(Order memory o) payable {
        IKCashFunding.ExactOut memory seed=IKCashFunding.ExactOut(o.weth,o.stable,500,address(this),o.quantity,msg.value,0);
        IKCashFunding(o.funding).exactOutputSingle{value:msg.value}(seed);
        IKCashToken src=IKCashToken(o.stable);
        IKCashToken dst=IKCashToken(o.target);
        require(src.approve(o.cash,o.quantity));
        uint256 initial=src.balanceOf(address(this));
        uint256 initialOut=dst.balanceOf(address(this));
        (bool ok,bytes memory ret)=o.cash.call(abi.encodeCall(IKCashSwap.swap,(o.pair,o.stable,o.target,o.quantity,o.minimum,block.timestamp+60)));
        uint256 spent=initial-src.balanceOf(address(this));
        uint256 net=dst.balanceOf(address(this))-initialOut;
        bytes memory observation=abi.encode(address(this),block.number,block.timestamp,ok,ret,initial,spent,net);
        assembly ("memory-safe") {return(add(observation,32),mload(observation))}
    }
}

