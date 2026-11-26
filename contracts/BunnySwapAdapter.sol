// SPDX-License-Identifier: MIT
// Обменивает точный вход через штатный RabbitRouter. На стороне ETH модуль сам
// разворачивает или заворачивает WETH. Кошелёк вызывает swapFrom напрямую или через
// шаг 29 ArbExecutor V5. Для своей сети модуль развёртывает владелец.
pragma solidity 0.8.37;

interface IRabbitRouter {
    function pair() external view returns (address);
    function token0() external view returns (address);
    function token1() external view returns (address);
    function swapExactETHForTokens(uint256 minimum,address to,uint256 deadline) external payable returns(uint256);
    function swapExactTokensForETH(uint256 input,uint256 minimum,address to,uint256 deadline) external returns(uint256);
}
interface IBunnyToken {
    function balanceOf(address who) external view returns(uint256);
    function allowance(address owner,address spender) external view returns(uint256);
    function transferFrom(address from,address to,uint256 amount) external returns(bool);
    function transfer(address to,uint256 amount) external returns(bool);
    function approve(address spender,uint256 amount) external returns(bool);
}
interface IBunnyWeth is IBunnyToken {
    function deposit() external payable;
    function withdraw(uint256 amount) external;
}
interface IBunnyPair {
    function isAllowedRouter(address router) external view returns(bool);
    function token0() external view returns(address);
    function token1() external view returns(address);
}

contract BunnySwapAdapter {
    IRabbitRouter public immutable router;
    bool transient private trading;

    error InvalidOrder();
    error IncompleteTransfer();

    constructor(IRabbitRouter market) {
        IBunnyPair pair=IBunnyPair(market.pair());
        if (!pair.isAllowedRouter(address(market)) || market.token0()!=pair.token0()
            || market.token1()!=pair.token1() || market.token0()==market.token1()) revert InvalidOrder();
        router=market;
    }

    receive() external payable {
        if (!trading || (msg.sender!=address(router) && msg.sender!=router.token1())) revert InvalidOrder();
    }

    function execute(address market,address input,address output,uint256 amount,uint256 minimum,uint256 deadline)
        external returns(uint256) {
        if (market!=router.pair() || minimum==0) revert InvalidOrder();
        return swapFrom(input,output,amount,minimum,msg.sender,deadline);
    }

    function swapFrom(address input,address output,uint256 amount,uint256 minimum,address to,uint256 deadline)
        public returns(uint256 received) {
        if (trading || block.timestamp>deadline || amount==0 || to==address(0) || to==address(this)
            || to==router.pair() || input==output) revert InvalidOrder();
        address friend=router.token0();
        address weth=router.token1();
        bool buy=input==weth && output==friend;
        if (!buy && !(input==friend && output==weth)) revert InvalidOrder();
        trading=true;
        IBunnyToken token=IBunnyToken(input);
        IBunnyToken payout=IBunnyToken(output);
        uint256 walletIn=token.balanceOf(msg.sender);
        uint256 baseIn=token.balanceOf(address(this));
        uint256 walletOut=payout.balanceOf(to);
        uint256 nativeBefore=address(this).balance;
        if (!token.transferFrom(msg.sender,address(this),amount)
            || token.balanceOf(address(this))-baseIn!=amount
            || walletIn-token.balanceOf(msg.sender)!=amount) revert IncompleteTransfer();
        if (buy) {
            IBunnyWeth(weth).withdraw(amount);
            router.swapExactETHForTokens{value:amount}(minimum,to,deadline);
        } else {
            if (!token.approve(address(router),amount)) revert IncompleteTransfer();
            uint256 paid=router.swapExactTokensForETH(amount,minimum,address(this),deadline);
            if (address(this).balance-nativeBefore!=paid || token.allowance(address(this),address(router))!=0)
                revert IncompleteTransfer();
            IBunnyWeth(weth).deposit{value:paid}();
            if (!payout.transfer(to,paid)) revert IncompleteTransfer();
        }
        received=payout.balanceOf(to)-walletOut;
        if (received==0 || received<minimum || token.balanceOf(address(this))!=baseIn
            || address(this).balance!=nativeBefore) revert IncompleteTransfer();
        trading=false;
    }
}
