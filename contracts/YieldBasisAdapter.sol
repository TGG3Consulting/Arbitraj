// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface IYieldBasisFactory {
    struct Market {
        address asset;
        address cryptopool;
        address amm;
        address lt;
        address oracle;
        address virtualPool;
        address staker;
    }
    function markets(uint256 index) external view returns (Market memory);
}
interface IYieldBasisPool {
    function coins(uint256 index) external view returns (address);
    function exchange(uint256 i,uint256 j,uint256 amount,uint256 minimum,address receiver) external returns (uint256);
}
interface IYieldBasisToken {
    function balanceOf(address owner) external view returns (uint256);
    function allowance(address owner,address spender) external view returns (uint256);
    function transferFrom(address from,address to,uint256 amount) external returns (bool);
    function approve(address spender,uint256 amount) external returns (bool);
    function deposit() external payable;
    function withdraw(uint256 amount) external;
}

/// Market keys pin the actual venue as well as its factory index and role.
/// index << 168 | role << 160 | venue. Roles 0/1/2 are CryptoPool/LEVAMM/VirtualPool.
contract YieldBasisAdapter {
    IYieldBasisFactory public immutable factory;
    address public immutable wrapped;
    bool transient private entered;

    struct Order {
        bytes32 market;
        address source;
        address target;
        uint256 amount;
        uint256 minimum;
        uint256 deadline;
    }
    event Paid(address indexed payer,address indexed recipient,address indexed venue,
               address source,address target,uint256 spent,uint256 received);

    constructor(IYieldBasisFactory original,address weth) {
        require(address(original).code.length > 0 && weth.code.length > 0 && address(original) != weth,"roles");
        factory = original;
        wrapped = weth;
    }
    receive() external payable { require(msg.sender == wrapped,"wrapper"); }

    function execute(Order calldata order) external payable returns (uint256) {
        return settle(order,msg.sender);
    }
    function swap(IBalancerVault.SingleSwap calldata step,IBalancerVault.FundManagement calldata funds,
                  uint256 minimum,uint256 deadline) external returns (uint256) {
        require(step.kind == 0 && step.userData.length == 0 && funds.sender == msg.sender &&
                !funds.fromInternalBalance && !funds.toInternalBalance &&
                step.assetIn != address(0) && step.assetOut != address(0),"step");
        return settle(Order(step.poolId,step.assetIn,step.assetOut,step.amount,minimum,deadline),funds.recipient);
    }
    function resolve(bytes32 key,address src,address dst) private view returns (IYieldBasisPool pool,uint256 i) {
        uint256 packed = uint256(key);
        IYieldBasisFactory.Market memory m = factory.markets(packed >> 168);
        uint8 role = uint8(packed >> 160);
        address venue = address(uint160(packed));
        require(role <= 2 && venue != address(0) &&
                venue == (role == 0 ? m.cryptopool : role == 1 ? m.amm : m.virtualPool),"market");
        pool = IYieldBasisPool(venue);
        address first = pool.coins(0);
        address second = pool.coins(1);
        require(src != dst && ((src == first && dst == second) || (src == second && dst == first)),"pair");
        i = src == first ? 0 : 1;
    }
    function cash(address asset,address who) private view returns (uint256) {
        return asset == address(0) ? who.balance : IYieldBasisToken(asset).balanceOf(who);
    }
    function tokenCall(address asset,bytes memory data) private {
        (bool ok,bytes memory result) = asset.call(data);
        require(ok && (result.length == 0 || abi.decode(result,(bool))),"token");
    }
    function settle(Order memory o,address recipient) private returns (uint256 received) {
        require(!entered && block.timestamp <= o.deadline && o.amount > 0 && o.minimum > 0 &&
                msg.value == (o.source == address(0) ? o.amount : 0) &&
                recipient != address(0) && recipient != address(this),"order");
        entered = true;
        address src = o.source == address(0) ? wrapped : o.source;
        address dst = o.target == address(0) ? wrapped : o.target;
        (IYieldBasisPool pool,uint256 i) = resolve(o.market,src,dst);
        require(address(pool) != recipient && address(pool) != msg.sender,"recipient");
        uint256 nativeBefore = address(this).balance-msg.value;
        uint256 moduleSource = cash(src,address(this));
        uint256 ownerSource = cash(o.source,msg.sender)+msg.value;
        uint256 recipientTarget = cash(o.target,recipient);
        uint256 wrappedBefore = o.target == address(0) ? cash(wrapped,address(this)) : 0;
        if (o.source == address(0)) IYieldBasisToken(wrapped).deposit{value:o.amount}();
        else tokenCall(src,abi.encodeCall(IYieldBasisToken.transferFrom,(msg.sender,address(this),o.amount)));
        require(cash(src,address(this)) == moduleSource+o.amount,"funding");
        tokenCall(src,abi.encodeCall(IYieldBasisToken.approve,(address(pool),o.amount)));
        // The virtual venue spends intermediate flash liquidity. Measure the recipient directly.
        pool.exchange(i,1-i,o.amount,o.minimum,o.target == address(0) ? address(this) : recipient);
        if (IYieldBasisToken(src).allowance(address(this),address(pool)) != 0) {
            tokenCall(src,abi.encodeCall(IYieldBasisToken.approve,(address(pool),0)));
            require(IYieldBasisToken(src).allowance(address(this),address(pool)) == 0,"allowance");
        }
        if (o.target == address(0)) {
            uint256 arrived = cash(wrapped,address(this))-wrappedBefore;
            IYieldBasisToken(wrapped).withdraw(arrived);
            (bool paid,) = recipient.call{value:arrived}("");
            require(paid && cash(wrapped,address(this)) == wrappedBefore,"native");
        }
        received = cash(o.target,recipient)-recipientTarget;
        require(received >= o.minimum && cash(o.source,msg.sender)+o.amount == ownerSource &&
                cash(src,address(this)) == moduleSource && address(this).balance == nativeBefore,"wallet bounds");
        emit Paid(msg.sender,recipient,address(pool),o.source,o.target,o.amount,received);
        entered = false;
    }
}
