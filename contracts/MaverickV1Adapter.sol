// SPDX-License-Identifier: MIT
// Обмен ERC-20 через зарегистрированный пул Maverick V1. Владелец размещает модуль
// с фабрикой своей сети. Кошелёк вызывает execute, ArbExecutor V5 — шаг 17 с адресом пула.
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface IMavOneFactory {
    function isFactoryPool(address pool) external view returns(bool);
}
interface IMavOnePool {
    function factory() external view returns(address);
    function tokenA() external view returns(address);
    function tokenB() external view returns(address);
    function swap(address recipient,uint256 amount,bool tokenAIn,bool exactOutput,uint256 sqrtPriceLimit,bytes calldata data)
        external returns(uint256 paid,uint256 out);
}
interface IMavOneToken {
    function balanceOf(address account) external view returns(uint256);
    function transferFrom(address payer,address recipient,uint256 amount) external returns(bool);
}

contract MaverickV1Adapter {
    IMavOneFactory public immutable factory;
    bool transient private entered;
    address transient private expectedPool;
    address transient private payer;
    address transient private assetIn;
    uint256 transient private debit;

    error InvalidOrder();
    error UnregisteredPool();
    error PartialInput();
    error MinimumOutput();

    constructor(IMavOneFactory registry) {
        if(address(registry).code.length==0)revert UnregisteredPool();
        factory=registry;
    }
    function execute(address pool,address tokenIn,address tokenOut,uint256 qty,uint256 minimum,uint256 deadline)
        external returns(uint256) {
        return settle(pool,tokenIn,tokenOut,qty,minimum,deadline,msg.sender);
    }
    function swap(IBalancerVault.SingleSwap calldata order,IBalancerVault.FundManagement calldata funds,
                  uint256 minimum,uint256 deadline) external payable returns(uint256) {
        if(msg.value!=0 || uint256(order.poolId)>>160!=0 || order.kind!=0 || order.userData.length!=0 ||
           funds.sender!=msg.sender || funds.fromInternalBalance || funds.toInternalBalance)revert InvalidOrder();
        return settle(address(uint160(uint256(order.poolId))),order.assetIn,order.assetOut,order.amount,minimum,deadline,funds.recipient);
    }
    function settle(address pool,address tokenIn,address tokenOut,uint256 qty,uint256 minimum,
                    uint256 deadline,address recipient) private returns(uint256 received) {
        if(entered || qty==0 || qty>type(uint128).max || minimum==0 || block.timestamp>deadline ||
           tokenIn==tokenOut || tokenIn==address(0) || tokenOut==address(0) || recipient==address(0) ||
           recipient==pool || recipient==address(this))revert InvalidOrder();
        entered=true;
        IMavOnePool market=IMavOnePool(pool);
        bool tokenAIn=tokenIn<tokenOut;
        if(!factory.isFactoryPool(pool) || market.factory()!=address(factory) ||
           market.tokenA()!=(tokenAIn?tokenIn:tokenOut) || market.tokenB()!=(tokenAIn?tokenOut:tokenIn))revert UnregisteredPool();
        uint256 initialIn=IMavOneToken(tokenIn).balanceOf(msg.sender);
        uint256 initialOut=IMavOneToken(tokenOut).balanceOf(recipient);
        payer=msg.sender;assetIn=tokenIn;debit=qty;expectedPool=pool;
        (uint256 paid,)=market.swap(recipient,qty,tokenAIn,false,0,hex"01");
        if(expectedPool!=address(0) || paid!=qty || initialIn-IMavOneToken(tokenIn).balanceOf(msg.sender)!=qty)revert PartialInput();
        received=IMavOneToken(tokenOut).balanceOf(recipient)-initialOut;
        if(received<minimum)revert MinimumOutput();
        entered=false;
    }
    function swapCallback(uint256 amountToPay,uint256 grossOut,bytes calldata context) external {
        if(!entered || msg.sender!=expectedPool || amountToPay!=debit || grossOut==0 ||
           context.length!=1 || context[0]!=0x01)revert PartialInput();
        expectedPool=address(0);
        (bool ok,bytes memory raw)=assetIn.call(abi.encodeCall(IMavOneToken.transferFrom,(payer,msg.sender,amountToPay)));
        if(!ok || (raw.length!=0 && !abi.decode(raw,(bool))))revert PartialInput();
    }
}
