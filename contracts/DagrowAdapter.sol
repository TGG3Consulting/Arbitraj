// SPDX-License-Identifier: MIT
// Модуль развёртывает владелец в Ethereum. Обменивает собственные средства плательщика через DAGrow SSP. Ограничивает окончательное списание
// и фактический приход адресату в той же транзакции. Тариф пула относится к адресу этого модуля.
pragma solidity 0.8.37;

interface IDagrowPool {
    function token0() external view returns (address);
    function token1() external view returns (address);
    function swapExactTokensForTokens(uint256 amount, uint256 minimum, address[] calldata path,
        address recipient, uint256 deadline) external returns (uint256[] memory);
    function swapTokensForExactTokens(uint256 desired, uint256 maximum, address[] calldata path,
        address recipient, uint256 deadline) external returns (uint256[] memory);
}
interface IDagrowCurrency {
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}
contract DagrowAdapter {
    IDagrowPool public immutable pool;
    address public immutable stable;
    address public immutable dagr;
    bool transient private busy;
    error InvalidOrder();
    error WalletAmount();
    event Settled(address indexed payer, address indexed recipient, address indexed input, address output,
        uint256 spent, uint256 received, bool byTarget);

    constructor(address venue, address stablecoin, address token) {
        if (venue.code.length==0 || stablecoin.code.length==0 || token.code.length==0 || stablecoin==token) revert InvalidOrder();
        pool=IDagrowPool(venue); stable=stablecoin; dagr=token;
        if (!((pool.token0()==stablecoin && pool.token1()==token) || (pool.token1()==stablecoin && pool.token0()==token))) revert InvalidOrder();
    }
    // ABI штатного шага V5 №29. Для EOA minimum и deadline подписаны прямо в вызове.
    function swapFrom(address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        external returns(uint256 received) {
        (,received)=_settle(input,output,amount,minimum,recipient,deadline,false);
    }
    function swapTo(address input,address output,uint256 desired,uint256 maximum,address recipient,uint256 deadline)
        external returns(uint256 spent,uint256 received) {
        return _settle(input,output,maximum,desired,recipient,deadline,true);
    }
    function _settle(address input,address output,uint256 maximum,uint256 minimum,address recipient,uint256 deadline,
                     bool byTarget) private returns(uint256 spent,uint256 received) {
        if (busy || block.timestamp>deadline || maximum==0 || recipient==address(0) || recipient==address(this)
            || !((input==stable && output==dagr)||(input==dagr && output==stable)) || (byTarget && minimum==0)) revert InvalidOrder();
        busy=true;
        IDagrowCurrency src=IDagrowCurrency(input);
        IDagrowCurrency dst=IDagrowCurrency(output);
        uint256 initial=src.balanceOf(address(this));
        uint256 payerBefore=src.balanceOf(msg.sender);
        uint256 receiverBefore=dst.balanceOf(recipient);
        if (src.allowance(address(this),address(pool))!=0) revert WalletAmount();
        _call(input,abi.encodeCall(src.transferFrom,(msg.sender,address(this),maximum)));
        if (src.balanceOf(address(this))!=initial+maximum) revert WalletAmount();
        _call(input,abi.encodeCall(src.approve,(address(pool),maximum)));
        address[] memory path=new address[](2); path[0]=input; path[1]=output;
        if (byTarget) pool.swapTokensForExactTokens(minimum,maximum,path,recipient,deadline);
        else pool.swapExactTokensForTokens(maximum,minimum,path,recipient,deadline);
        if (src.allowance(address(this),address(pool))!=0) _call(input,abi.encodeCall(src.approve,(address(pool),0)));
        uint256 remainder=src.balanceOf(address(this))-initial;
        if (remainder>maximum || (!byTarget && remainder!=0)) revert WalletAmount();
        if (remainder!=0) _call(input,abi.encodeCall(src.transfer,(msg.sender,remainder)));
        spent=payerBefore-src.balanceOf(msg.sender);
        received=dst.balanceOf(recipient)-receiverBefore;
        if (spent==0 || spent>maximum || (!byTarget && spent!=maximum) || received==0 || received<minimum
            || src.balanceOf(address(this))!=initial || src.allowance(address(this),address(pool))!=0) revert WalletAmount();
        emit Settled(msg.sender,recipient,input,output,spent,received,byTarget);
        busy=false;
    }
    function _call(address target,bytes memory data) private {
        (bool ok,bytes memory returned)=target.call(data);
        if (!ok || (returned.length!=0 && !abi.decode(returned,(bool)))) revert WalletAmount();
    }
}
