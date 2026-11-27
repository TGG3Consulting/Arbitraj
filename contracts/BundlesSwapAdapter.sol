// SPDX-License-Identifier: MIT
// Модуль развёртывает владелец в Ethereum для проверенного Router Bundles Swap.
// В поручении указывается физическая пара. Проверяются расход входа, комиссия BUN
// и чистое поступление получателю. В V5 шаг 17 использует средства одного входа.
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface IBundlesCurrency {
    function balanceOf(address account) external view returns (uint256);
    function transfer(address to, uint256 qty) external returns (bool);
    function transferFrom(address from, address to, uint256 qty) external returns (bool);
    function approve(address spender, uint256 qty) external returns (bool);
    function allowance(address owner, address spender) external view returns (uint256);
}

interface IBundlesPair {
    function factory() external view returns (address);
    function token0() external view returns (address);
    function token1() external view returns (address);
    function fee() external view returns (uint16);
}

interface IBundlesFactory {
    function getPair(address a, address b, uint16 fee) external view returns (address);
}

interface IBundlesRouter {
    struct Exact { uint256 amount; uint256 minimum; uint256 maxFee; address[] path; uint16[] fees; address to; uint256 deadline; }
    function factory() external view returns(address);
    function weth() external view returns(address);
    function protocolToken() external view returns(address);
    function pairFor(address a,address b,uint16 fee) external view returns(address);
    function getAmountsOut(uint256 qty,address[] calldata path,uint16[] calldata fees) external view returns(uint256[] memory);
    function swapExactTokensForTokens(Exact calldata order) external returns(uint256[] memory);
    function swapExactWETHForTokens(uint256 qty,address output,uint16 fee,address to,uint256 deadline,uint256 minimum) external returns(uint256);
    function swapExactTokensForWETH(address input,uint16 fee,address to,uint256 deadline,uint256 qty,uint256 minimum) external returns(uint256);
}

contract BundlesSwapAdapter {
    IBundlesRouter public immutable router;
    bool transient private entered;

    struct Order { address pool; address input; address output; address recipient; uint256 amount; uint256 minimum; uint256 deadline; }
    event Filled(address indexed payer,address indexed pool,address indexed recipient,address input,address output,
                 uint256 debit,uint256 credit,uint256 feeDebit);

    constructor(IBundlesRouter venue) {
        require(block.chainid==1 && address(venue).code.length!=0 && venue.factory().code.length!=0
            && venue.weth().code.length!=0 && venue.protocolToken().code.length!=0,"venue");
        router=venue;
    }

    function execute(Order calldata order,uint256 maxFeeDebit) external returns(uint256) {
        return _settle(order,maxFeeDebit);
    }

    function swap(IBalancerVault.SingleSwap calldata order,IBalancerVault.FundManagement calldata funds,
                  uint256 minimum,uint256 deadline) external payable returns(uint256) {
        require(msg.value==0 && order.kind==0 && order.userData.length==0 && funds.sender==msg.sender
            && !funds.fromInternalBalance && !funds.toInternalBalance && uint256(order.poolId)>>160==0,"order");
        return _settle(Order(address(uint160(uint256(order.poolId))),order.assetIn,order.assetOut,
                             funds.recipient,order.amount,minimum,deadline),0);
    }

    function _call(address asset,bytes memory data) private {
        (bool ok,bytes memory value)=asset.call(data);
        require(asset.code.length!=0 && ok && (value.length==0 || abi.decode(value,(bool))),"token");
    }

    function _allow(IBundlesCurrency asset,uint256 value) private {
        if(asset.allowance(address(this),address(router))!=0)
            _call(address(asset),abi.encodeCall(asset.approve,(address(router),0)));
        if(value!=0) _call(address(asset),abi.encodeCall(asset.approve,(address(router),value)));
    }

    function _pull(IBundlesCurrency asset,uint256 value) private returns(uint256 delta) {
        uint256 base=asset.balanceOf(address(this));
        _call(address(asset),abi.encodeCall(asset.transferFrom,(msg.sender,address(this),value)));
        delta=asset.balanceOf(address(this))-base;
        require(delta>0 && delta<=value,"input");
    }

    function _settle(Order memory o,uint256 cap) private returns(uint256 credit) {
        require(!entered && o.amount!=0 && block.timestamp<=o.deadline && o.input!=o.output && o.pool.code.length!=0,"order");
        require(o.recipient!=address(0) && o.recipient!=address(this) && o.recipient!=o.pool && o.recipient!=address(router),"recipient");
        entered=true;
        IBundlesPair pair=IBundlesPair(o.pool);
        uint16 tier=pair.fee();
        address factory=router.factory();
        require(pair.factory()==factory && ((pair.token0()==o.input && pair.token1()==o.output) ||
                (pair.token1()==o.input && pair.token0()==o.output)) &&
                IBundlesFactory(factory).getPair(o.input,o.output,tier)==o.pool && router.pairFor(o.input,o.output,tier)==o.pool,"pair");
        IBundlesCurrency input=IBundlesCurrency(o.input);
        IBundlesCurrency output=IBundlesCurrency(o.output);
        IBundlesCurrency bun=IBundlesCurrency(router.protocolToken());
        uint256 base=input.balanceOf(address(this));
        uint256 beforeIn=input.balanceOf(msg.sender);
        uint256 beforeOut=output.balanceOf(o.recipient);
        uint256 amount=_pull(input,o.amount);
        _allow(input,amount);
        uint256 feeDebit;
        if(address(bun)==o.input || address(bun)==o.output) {
            require(cap==0,"embedded");
            // The pair charges its own token as part of the quoted input or output.
            address[] memory path=new address[](2); path[0]=o.input; path[1]=o.output;
            uint16[] memory fees=new uint16[](1); fees[0]=tier;
            uint256 limit=address(bun)==o.input ? amount*tier/(1000+uint256(tier)) :
                    router.getAmountsOut(amount,path,fees)[1]*tier/1000;
            router.swapExactTokensForTokens(IBundlesRouter.Exact(amount,o.minimum,limit,path,fees,o.recipient,o.deadline));
        } else if(cap!=0) {
            uint256 beforeFee=bun.balanceOf(msg.sender);
            uint256 baseFee=bun.balanceOf(address(this));
            require(_pull(bun,cap)==cap,"BUN");
            _allow(bun,cap);
            address[] memory path=new address[](2); path[0]=o.input; path[1]=o.output;
            uint16[] memory fees=new uint16[](1); fees[0]=tier;
            router.swapExactTokensForTokens(IBundlesRouter.Exact(amount,o.minimum,cap,path,fees,o.recipient,o.deadline));
            _allow(bun,0);
            uint256 remainder=bun.balanceOf(address(this))-baseFee;
            require(remainder<=cap,"fee");
            if(remainder>0) _call(address(bun),abi.encodeCall(bun.transfer,(msg.sender,remainder)));
            feeDebit=beforeFee-bun.balanceOf(msg.sender);
            require(feeDebit<=cap && bun.balanceOf(address(this))==baseFee,"fee debit");
        } else {
            address wrapped=router.weth();
            if(o.input==wrapped) router.swapExactWETHForTokens(amount,o.output,tier,o.recipient,o.deadline,o.minimum);
            else {
                require(o.output==wrapped,"WETH or BUN fee budget");
                router.swapExactTokensForWETH(o.input,tier,o.recipient,o.deadline,amount,o.minimum);
            }
        }
        _allow(input,0);
        uint256 debit=beforeIn-input.balanceOf(msg.sender);
        credit=output.balanceOf(o.recipient)-beforeOut;
        require(input.balanceOf(address(this))==base && debit==o.amount && credit>0 && credit>=o.minimum,"wallet");
        emit Filled(msg.sender,o.pool,o.recipient,o.input,o.output,debit,credit,feeDebit);
        entered=false;
    }
}
