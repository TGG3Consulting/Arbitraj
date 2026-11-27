// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface IBFactoryV1 {
    function isBPool(address pool) external view returns (bool);
}
interface IBPoolV1 {
    function swapExactAmountIn(address,uint256,address,uint256,uint256) external returns (uint256,uint256);
    function swapExactAmountOut(address,uint256,address,uint256,uint256) external returns (uint256,uint256);
}
interface IBTokenV1 {
    function balanceOf(address owner) external view returns (uint256);
    function allowance(address owner,address spender) external view returns (uint256);
    function approve(address spender,uint256 amount) external returns (bool);
    function transfer(address recipient,uint256 amount) external returns (bool);
    function transferFrom(address owner,address recipient,uint256 amount) external returns (bool);
    function deposit() external payable;
    function withdraw(uint256 amount) external;
}

/// A BPool has no deadline. Check it atomically and measure both wallets and the pool's underlying.
contract BalancerV1Adapter {
    IBFactoryV1 public immutable factory;
    address public immutable wrapped;
    bool transient private entered;

    struct Order {
        address pool;
        address assetIn;
        address assetOut;
        uint256 maximum;
        uint256 minimum;
        uint256 maxPrice;
        uint256 deadline;
        bool exactOutput;
    }
    struct Before {
        uint256 ethBalance;
        uint256 moduleSource;
        uint256 moduleTarget;
        uint256 ownerInput;
        uint256 recipientOutput;
        uint256 poolInput;
    }
    event Paid(address indexed payer,address indexed recipient,address indexed pool,address assetIn,address assetOut,
               bool exactOutput,uint256 spent,uint256 received);

    constructor(IBFactoryV1 original,address weth) {
        require(address(original).code.length > 0 && weth.code.length > 0 && address(original) != weth,"roles");
        factory = original;
        wrapped = weth;
    }
    receive() external payable { require(msg.sender == wrapped,"wrapper"); }

    function execute(Order calldata order) external payable returns (uint256 spent,uint256 received) {
        return settle(order,msg.sender);
    }

    function swap(IBalancerVault.SingleSwap calldata route,IBalancerVault.FundManagement calldata funds,
                  uint256 minimum,uint256 deadline) external returns (uint256 received) {
        require(route.kind == 0 && route.userData.length == 0 && funds.sender == msg.sender &&
                !funds.fromInternalBalance && !funds.toInternalBalance && uint256(route.poolId) <= type(uint160).max &&
                route.assetIn != address(0) && route.assetOut != address(0),"step");
        (,received) = settle(Order(address(uint160(uint256(route.poolId))),route.assetIn,route.assetOut,
                                   route.amount,minimum,type(uint256).max,deadline,false),funds.recipient);
    }

    function cash(address asset,address owner) private view returns (uint256) {
        return asset == address(0) ? owner.balance : IBTokenV1(asset).balanceOf(owner);
    }
    function tokenCall(address asset,bytes memory data) private {
        (bool ok,bytes memory result) = asset.call(data);
        require(ok && (result.length == 0 || abi.decode(result,(bool))),"token");
    }
    function pay(address asset,address recipient,uint256 amount) private {
        if (asset == address(0)) {
            IBTokenV1(wrapped).withdraw(amount);
            (bool ok,) = recipient.call{value:amount}("");
            require(ok,"native payment");
        } else {
            tokenCall(asset,abi.encodeCall(IBTokenV1.transfer,(recipient,amount)));
        }
    }

    function settle(Order memory o,address recipient) private returns (uint256 spent,uint256 received) {
        require(!entered && block.timestamp <= o.deadline && o.maximum > 0 && o.minimum > 0 && o.maxPrice > 0 &&
                msg.value == (o.assetIn == address(0) ? o.maximum : 0) &&
                recipient != address(0) && recipient != address(this) && recipient != o.pool && msg.sender != o.pool,"order");
        entered = true;
        address src = o.assetIn == address(0) ? wrapped : o.assetIn;
        address dst = o.assetOut == address(0) ? wrapped : o.assetOut;
        require(src != dst && factory.isBPool(o.pool),"market");
        Before memory old = Before(address(this).balance-msg.value,cash(src,address(this)),cash(dst,address(this)),
            cash(o.assetIn,msg.sender)+msg.value,cash(o.assetOut,recipient),cash(src,o.pool));
        if (o.assetIn == address(0)) {
            IBTokenV1(wrapped).deposit{value:o.maximum}();
        } else {
            tokenCall(src,abi.encodeCall(IBTokenV1.transferFrom,(msg.sender,address(this),o.maximum)));
        }
        require(cash(src,address(this)) == old.moduleSource+o.maximum,"input");
        tokenCall(src,abi.encodeCall(IBTokenV1.approve,(o.pool,o.maximum)));
        IBPoolV1 pool = IBPoolV1(o.pool);
        uint256 nominal;
        if (o.exactOutput) {
            (spent,) = pool.swapExactAmountOut(src,o.maximum,dst,o.minimum,o.maxPrice);
            nominal = o.minimum;
        } else {
            (nominal,) = pool.swapExactAmountIn(src,o.maximum,dst,o.minimum,o.maxPrice);
            spent = o.maximum;
        }
        require(spent > 0 && spent <= o.maximum && cash(src,o.pool) == old.poolInput+spent &&
                cash(src,address(this)) == old.moduleSource+o.maximum-spent,"pool payment");
        if (IBTokenV1(src).allowance(address(this),o.pool) != 0) {
            tokenCall(src,abi.encodeCall(IBTokenV1.approve,(o.pool,0)));
            require(IBTokenV1(src).allowance(address(this),o.pool) == 0,"allowance");
        }
        uint256 arrival = cash(dst,address(this))-old.moduleTarget;
        require(arrival >= o.minimum && arrival <= nominal,"pool output");
        if (o.maximum > spent) pay(o.assetIn,msg.sender,o.maximum-spent);
        pay(o.assetOut,recipient,arrival);
        received = cash(o.assetOut,recipient)-old.recipientOutput;
        require(received >= o.minimum && cash(o.assetIn,msg.sender)+spent == old.ownerInput &&
                cash(src,address(this)) == old.moduleSource && cash(dst,address(this)) == old.moduleTarget &&
                address(this).balance == old.ethBalance,"wallet bounds");
        emit Paid(msg.sender,recipient,o.pool,o.assetIn,o.assetOut,o.exactOutput,spent,received);
        entered = false;
    }
}
