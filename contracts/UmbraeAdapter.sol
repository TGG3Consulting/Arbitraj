// SPDX-License-Identifier: MIT
// Base. Владелец кошелька развёртывает один модуль для рынков двух фабрик Umbrae.
// Кошелёк вызывает execute, ArbExecutor V5 — шаг 17 с адресом конкретного пула.
// Защита проверяет реестр, код прокси и реализацию beacon, полный вход, срок и чистую выплату.
pragma solidity 0.8.37;
import {IBalancerVault} from "./IPools.sol";

interface IUmbraeRegistry {
    function beacon() external view returns(address);
    function getPair(address,address) external view returns(address);
    function getPair(address,address,uint16,uint16) external view returns(address);
    function getPairBaseFactor(address) external view returns(uint16);
}
interface IUmbraeBeacon {
    function implementation() external view returns(address);
}
interface IUmbraePair {
    function tokenX() external view returns(address);
    function tokenY() external view returns(address);
    function binStep() external view returns(uint16);
    function swap(bool,address,uint256,uint256) external returns(uint256);
    function swap(uint256,bool,uint256,address) external returns(uint256);
}
interface IUmbraeToken {
    function balanceOf(address) external view returns(uint256);
    function transferFrom(address,address,uint256) external returns(bool);
    function approve(address,uint256) external returns(bool);
}
contract UmbraeAdapter {
    IUmbraeRegistry public immutable dlmm;
    IUmbraeRegistry public immutable damm;
    IUmbraeBeacon private immutable binsBeacon;
    IUmbraeBeacon private immutable constantBeacon;
    address private immutable binsImpl;
    address private immutable constantImpl;
    bytes32 private immutable binsProxy;
    bytes32 private immutable constantProxy;
    bool transient private entered;
    error InvalidOrder();
    error UnknownPool();
    error IncompletePayment();
    error TokenCall();

    constructor(IUmbraeRegistry bins,IUmbraeRegistry cp,address binsSample,address cpSample) {
        dlmm=bins; damm=cp;
        binsBeacon=IUmbraeBeacon(bins.beacon());
        constantBeacon=IUmbraeBeacon(cp.beacon());
        binsImpl=binsBeacon.implementation();
        constantImpl=constantBeacon.implementation();
        if (binsImpl.code.length==0 || constantImpl.code.length==0 || binsSample.code.length==0 || cpSample.code.length==0)
            revert UnknownPool();
        binsProxy=binsSample.codehash; constantProxy=cpSample.codehash;
        if (binsProxy==constantProxy || !registered(binsSample,true) || !registered(cpSample,false)) revert UnknownPool();
    }

    function registered(address p,bool bins) private view returns(bool) {
        IUmbraePair pair=IUmbraePair(p);
        address x=pair.tokenX(); address y=pair.tokenY();
        if (p.codehash!=(bins?binsProxy:constantProxy) || x==address(0) || x==y) return false;
        if (bins) return dlmm.beacon()==address(binsBeacon) && binsBeacon.implementation()==binsImpl &&
            dlmm.getPair(x,y,pair.binStep(),dlmm.getPairBaseFactor(p))==p;
        return damm.beacon()==address(constantBeacon) && constantBeacon.implementation()==constantImpl && damm.getPair(x,y)==p;
    }

    function execute(address pool,address input,address output,uint256 amount,uint256 minimum,uint256 deadline)
        external returns(uint256) {
        return settle(pool,input,output,amount,minimum,deadline,msg.sender);
    }

    function swap(IBalancerVault.SingleSwap calldata order,IBalancerVault.FundManagement calldata funds,
                  uint256 minimum,uint256 deadline) external payable returns(uint256) {
        if(msg.value!=0 || uint256(order.poolId)>>160!=0 || order.kind!=0 || order.userData.length!=0 ||
           funds.sender!=msg.sender || funds.fromInternalBalance || funds.toInternalBalance) revert InvalidOrder();
        return settle(address(uint160(uint256(order.poolId))),order.assetIn,order.assetOut,order.amount,minimum,deadline,funds.recipient);
    }

    function tokenCall(address token,bytes memory data) private {
        (bool ok,bytes memory ret)=token.call(data);
        if(!ok || (ret.length!=0 && !abi.decode(ret,(bool)))) revert TokenCall();
    }

    function settle(address p,address input,address output,uint256 amount,uint256 minimum,uint256 deadline,address to)
        private returns(uint256 received) {
        if(entered || block.timestamp>deadline || amount==0 || minimum==0 || to==address(0) ||
           to==p || to==address(this) || input==output) revert InvalidOrder();
        entered=true;
        bool bins=p.codehash==binsProxy;
        if(!registered(p,bins)) revert UnknownPool();
        IUmbraePair pair=IUmbraePair(p);
        bool forward=pair.tokenX()==input;
        if((forward?pair.tokenY():pair.tokenX())!=output || (!forward && pair.tokenY()!=input)) revert UnknownPool();
        IUmbraeToken token=IUmbraeToken(input);
        uint256 beforePayer=token.balanceOf(msg.sender);
        uint256 beforeOut=IUmbraeToken(output).balanceOf(to);
        address destination=bins?p:address(this);
        uint256 beforeDestination=token.balanceOf(destination);
        tokenCall(input,abi.encodeCall(token.transferFrom,(msg.sender,destination,amount)));
        if(token.balanceOf(destination)-beforeDestination!=amount) revert IncompletePayment();
        if(bins) pair.swap(forward,to,amount,minimum);
        else {
            tokenCall(input,abi.encodeCall(token.approve,(p,amount)));
            pair.swap(amount,forward,minimum,to);
            tokenCall(input,abi.encodeCall(token.approve,(p,0)));
            if(token.balanceOf(address(this))!=beforeDestination) revert IncompletePayment();
        }
        if(beforePayer-token.balanceOf(msg.sender)!=amount) revert IncompletePayment();
        received=IUmbraeToken(output).balanceOf(to)-beforeOut;
        if(received<minimum) revert IncompletePayment();
        entered=false;
    }
}
