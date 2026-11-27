// SPDX-License-Identifier: MIT
// Ethereum MotoSwapFactory and FeeRouter: https://docs.motoswap.org/dev/addresses
// The public fee path settles to recipient. The module scopes its approval to the amount actually
// received, enforces the wallet's debit and final credit even if an implementation changes, and
// exposes the same physical pair to mixed V5 routes through the Balancer step ABI.
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface IMotoAsset {
    function balanceOf(address) external view returns (uint256);
    function allowance(address,address) external view returns (uint256);
    function approve(address,uint256) external returns (bool);
    function transferFrom(address,address,uint256) external returns (bool);
}
interface IMotoFactory {
    function getPair(address,address) external view returns (address);
}
interface IMotoPair {
    function factory() external view returns (address);
    function token0() external view returns (address);
    function token1() external view returns (address);
}
interface IMotoFeeRouter {
    function router() external view returns (address);
    function swapExactTokensForTokensSupportingFeeOnTransferTokens(
        uint256,uint256,address[] calldata,address,uint256
    ) external returns (uint256 delivered);
}
interface IMotoCore {
    function factory() external view returns (address);
}

contract MotoswapWallet {
    IMotoFactory public immutable factory;
    IMotoFeeRouter public immutable router;
    bool transient private busy;
    event Paid(address indexed pool,address indexed payer,address indexed recipient,address input,address output,uint256 spent,uint256 received);
    event PathPaid(bytes32 indexed route,address indexed payer,address indexed recipient,address input,address output,uint256 spent,uint256 received);
    error InvalidMarket();
    error InvalidOrder();
    error FundingMismatch();
    error CreditMismatch();

    constructor(IMotoFactory registry, IMotoFeeRouter publicRouter) {
        if(address(registry).code.length==0 || address(publicRouter).code.length==0 ||
           IMotoCore(publicRouter.router()).factory()!=address(registry)) revert InvalidMarket();
        factory=registry;
        router=publicRouter;
    }

    function execute(address pool,address input,address output,uint256 amount,uint256 minimum,uint256 deadline)
        external returns (uint256) {
        return settle(pool,input,output,amount,minimum,deadline,msg.sender);
    }

    function swap(IBalancerVault.SingleSwap calldata order, IBalancerVault.FundManagement calldata funds,
                  uint256 minimum,uint256 deadline) external payable returns (uint256) {
        if(msg.value!=0 || order.kind!=0 || uint256(order.poolId)>>160!=0 || order.userData.length!=0 ||
           funds.sender!=msg.sender || funds.fromInternalBalance || funds.toInternalBalance) revert InvalidOrder();
        return settle(address(uint160(uint256(order.poolId))),order.assetIn,order.assetOut,
                      order.amount,minimum,deadline,funds.recipient);
    }

    function tokenCall(IMotoAsset token,bytes memory data) private {
        (bool ok,bytes memory r)=address(token).call(data);
        if(!ok || (r.length!=0 && !abi.decode(r,(bool)))) revert FundingMismatch();
    }

    struct Opening {uint256 payerIn;uint256 recipientOut;uint256 moduleIn;uint256 moduleOut;}
    function settle(address pool,address input,address output,uint256 amount,uint256 minimum,uint256 deadline,address recipient)
        private returns(uint256 net) {
        address[] memory path=new address[](2);
        path[0]=input;
        path[1]=output;
        validatePair(pool,input,output,recipient);
        net=trade(path,amount,minimum,deadline,recipient);
        emit Paid(pool,msg.sender,recipient,input,output,amount,net);
    }

    // Версия ABI executePath, которую проверяет IMoto.pathVersion в pools/motoswap_frame.rs.
    uint256 private constant PATH_VERSION = 1;
    function pathVersion() external pure returns(uint256){return PATH_VERSION;}

    function executePath(address[] calldata pools,address[] calldata path,uint256 amount,uint256 minimum,uint256 deadline)
        external returns(uint256 net) {
        if(pools.length<2 || path.length!=pools.length+1) revert InvalidMarket();
        for(uint256 i;i<pools.length;++i) validatePair(pools[i],path[i],path[i+1],msg.sender);
        net=trade(path,amount,minimum,deadline,msg.sender);
        emit PathPaid(keccak256(abi.encode(pools,path)),msg.sender,msg.sender,path[0],path[path.length-1],amount,net);
    }

    function validatePair(address pool,address input,address output,address recipient) private view {
        if(pool.code.length==0 || recipient==pool || input==output || factory.getPair(input,output)!=pool ||
           IMotoPair(pool).factory()!=address(factory)) revert InvalidMarket();
        (address a,address b)=input<output ? (input,output) : (output,input);
        if(IMotoPair(pool).token0()!=a || IMotoPair(pool).token1()!=b) revert InvalidMarket();
    }

    function trade(address[] memory path,uint256 amount,uint256 minimum,uint256 deadline,address recipient)
        private returns(uint256 net) {
        address input=path[0];
        address output=path[path.length-1];
        if(busy || amount==0 || minimum==0 || block.timestamp>deadline || input==output ||
           input.code.length==0 || output.code.length==0 || recipient==address(0) ||
           recipient==address(this) || recipient==address(router)) revert InvalidOrder();
        busy=true;
        if(IMotoCore(router.router()).factory()!=address(factory)) revert InvalidMarket();
        IMotoAsset src=IMotoAsset(input);
        IMotoAsset dst=IMotoAsset(output);
        Opening memory start=Opening(src.balanceOf(msg.sender),dst.balanceOf(recipient),src.balanceOf(address(this)),dst.balanceOf(address(this)));
        if(src.allowance(address(this),address(router))!=0) revert FundingMismatch();
        tokenCall(src,abi.encodeCall(src.transferFrom,(msg.sender,address(this),amount)));
        uint256 credited=src.balanceOf(address(this))-start.moduleIn;
        if(credited==0 || credited>amount) revert FundingMismatch();
        tokenCall(src,abi.encodeCall(src.approve,(address(router),credited)));
        router.swapExactTokensForTokensSupportingFeeOnTransferTokens(credited,minimum,path,recipient,deadline);
        tokenCall(src,abi.encodeCall(src.approve,(address(router),0)));
        net=dst.balanceOf(recipient)-start.recipientOut;
        if(start.payerIn-src.balanceOf(msg.sender)!=amount || src.balanceOf(address(this))!=start.moduleIn ||
           dst.balanceOf(address(this))!=start.moduleOut || src.allowance(address(this),address(router))!=0)
            revert FundingMismatch();
        if(net<minimum) revert CreditMismatch();
        busy=false;
    }
}

