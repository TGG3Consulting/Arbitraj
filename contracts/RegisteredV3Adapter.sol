// SPDX-License-Identifier: MIT
// One registered exact-input V3 pool, paid from the original caller in its callback.
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface IRegisteredV3Factory {
    function getPool(address a,address b,uint24 fee) external view returns(address);
}

interface IRegisteredV3Pool {
    function factory() external view returns(address);
    function token0() external view returns(address);
    function token1() external view returns(address);
    function fee() external view returns(uint24);
    function swap(address recipient,bool zeroForOne,int256 amountSpecified,uint160 limit,bytes calldata data)
        external returns(int256,int256);
}

interface IFundedV3Token {
    function balanceOf(address who) external view returns(uint256);
    function transferFrom(address from,address to,uint256 amount) external returns(bool);
}

abstract contract RegisteredV3Adapter {
    IRegisteredV3Factory public immutable factory;
    // Remains locked across ERC20 reads/transfers, callback and receipt checks.
    bool transient private trading;
    address transient private expectedPool;
    address transient private inputToken;
    address transient private payer;
    uint256 transient private expectedDebit;
    bool transient private inputIsToken0;

    error InvalidOrder();
    error UnknownPool();
    error IncompletePayment();
    error MissingOutput();

    constructor(IRegisteredV3Factory registry) {
        if(address(registry).code.length==0)revert UnknownPool();
        factory=registry;
    }

    function execute(address pool,address input,address output,uint256 amount,uint256 minimum,uint256 deadline)
        external returns(uint256) {
        return settle(pool,input,output,amount,minimum,deadline,msg.sender);
    }

    // ArbExecutor V5 step 17 supplies an exact pool id. Its step 29 supplies only two tokens,
    // which cannot distinguish fee tiers. The id here is a right-aligned pool address.
    // This contract implements the one GIVEN_IN wire call and never looks up a Balancer pool.
    function swap(IBalancerVault.SingleSwap calldata order,IBalancerVault.FundManagement calldata funds,
                  uint256 minimum,uint256 deadline) external payable returns(uint256) {
        if(msg.value!=0 || uint256(order.poolId)>>160!=0 || order.kind!=0 || order.userData.length!=0
            || funds.sender!=msg.sender || funds.fromInternalBalance || funds.toInternalBalance)revert InvalidOrder();
        return settle(address(uint160(uint256(order.poolId))),order.assetIn,order.assetOut,order.amount,minimum,deadline,funds.recipient);
    }

    function settle(address where,address input,address output,uint256 qty,uint256 minimum,uint256 deadline,address recipient)
        private returns(uint256 received) {
        if(trading || qty==0 || qty>uint256(type(int256).max) || minimum==0 || block.timestamp>deadline
            || input==output || input==address(0) || output==address(0) || recipient==address(0)
            || recipient==where || recipient==address(this))revert InvalidOrder();
        trading=true;
        IRegisteredV3Pool pool=IRegisteredV3Pool(where);
        bool zeroIn=input<output;
        if(pool.token0()!=(zeroIn?input:output) || pool.token1()!=(zeroIn?output:input)
            || pool.factory()!=address(factory) || factory.getPool(input,output,pool.fee())!=where)revert UnknownPool();
        uint256 beforeIn=IFundedV3Token(input).balanceOf(msg.sender);
        uint256 beforeOut=IFundedV3Token(output).balanceOf(recipient);
        expectedPool=where; inputToken=input; payer=msg.sender; expectedDebit=qty; inputIsToken0=zeroIn;
        (int256 d0,int256 d1)=pool.swap(recipient,zeroIn,int256(qty),zeroIn?4295128740:1461446703485210103287273052203988822378723970341,"");
        if(expectedPool!=address(0) || (zeroIn?d0:d1)!=int256(qty)
            || beforeIn-IFundedV3Token(input).balanceOf(msg.sender)!=qty)revert IncompletePayment();
        received=IFundedV3Token(output).balanceOf(recipient)-beforeOut;
        if(received<minimum)revert MissingOutput();
        trading=false;
    }

    function pay(int256 d0,int256 d1,bytes calldata data) internal {
        if(!trading || expectedPool!=msg.sender || data.length!=0)revert IncompletePayment();
        int256 debit=inputIsToken0?d0:d1;
        if(debit<=0 || uint256(debit)!=expectedDebit || (inputIsToken0?d1:d0)>=0)revert IncompletePayment();
        expectedPool=address(0);
        (bool ok,bytes memory ret)=inputToken.call(abi.encodeCall(IFundedV3Token.transferFrom,(payer,msg.sender,uint256(debit))));
        if(!ok || (ret.length!=0 && !abi.decode(ret,(bool))))revert IncompletePayment();
    }
}
