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
    address transient private nativeSender;

    error InvalidOrder();
    error IncompleteTransfer();

    constructor(IRabbitRouter market) {
        IBunnyPair pair=IBunnyPair(market.pair());
        if (!pair.isAllowedRouter(address(market)) || market.token0()!=pair.token0()
            || market.token1()!=pair.token1() || market.token0()==market.token1()) revert InvalidOrder();
        router=market;
    }

    receive() external payable {
        if (msg.sender!=nativeSender) revert InvalidOrder();
    }

    // FRIEND возвращает пустые данные при успешных transferFrom и approve. WETH возвращает true.
    // Ошибка вызова, false и некорректный ответ откатывают обмен.
    function checkedCall(address token,bytes memory data) private {
        (bool ok,bytes memory result)=token.call(data);
        if (!ok || (result.length!=0 && !abi.decode(result,(bool)))) revert IncompleteTransfer();
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
        checkedCall(input,abi.encodeCall(IBunnyToken.transferFrom,(msg.sender,address(this),amount)));
        if (token.balanceOf(address(this))-baseIn!=amount
            || walletIn-token.balanceOf(msg.sender)!=amount) revert IncompleteTransfer();
        if (buy) {
            nativeSender=weth;
            IBunnyWeth(weth).withdraw(amount);
            nativeSender=address(0);
            router.swapExactETHForTokens{value:amount}(minimum,to,deadline);
        } else {
            checkedCall(input,abi.encodeCall(IBunnyToken.approve,(address(router),amount)));
            nativeSender=address(router);
            uint256 paid=router.swapExactTokensForETH(amount,minimum,address(this),deadline);
            nativeSender=address(0);
            if (address(this).balance-nativeBefore!=paid || token.allowance(address(this),address(router))!=0)
                revert IncompleteTransfer();
            IBunnyWeth(weth).deposit{value:paid}();
            checkedCall(output,abi.encodeCall(IBunnyToken.transfer,(to,paid)));
        }
        received=payout.balanceOf(to)-walletOut;
        if (received==0 || received<minimum || token.balanceOf(address(this))!=baseIn
            || address(this).balance!=nativeBefore) revert IncompleteTransfer();
        trading=false;
    }
}
