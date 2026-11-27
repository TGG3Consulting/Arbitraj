// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

import {IERC20, IBalancerVault} from "./IPools.sol";

interface IBrokerRegistry {
    function getBrokerbot(address base,address share) external view returns(address);
}
interface IEquityBroker {
    function base() external view returns(address);
    function token() external view returns(address);
    function paymenthub() external view returns(address);
    function settings() external view returns(uint256);
    function getBuyPrice(uint256 shares) external view returns(uint256);
    function getShares(uint256 cash) external view returns(uint256);
}
interface ISharePayment {
    function payAndNotify(address broker,uint256 cash,bytes calldata ref) external returns(uint256);
}
interface ICallbackShare {
    function transferAndCall(address recipient,uint256 shares,bytes calldata ref) external returns(bool);
}

/// Pays a signed broker only with the caller's assets. Issued shares are integral units.
contract AktionariatWallet {
    IBrokerRegistry public immutable registry;
    address public immutable issuerHub;
    bool private transient entered;

    struct Order {
        address broker;
        address source;
        address target;
        uint256 budget;
        uint256 minimum;
        uint256 deadline;
    }
    event Paid(address indexed payer,address indexed recipient,address indexed broker,
               address source,address target,uint256 spent,uint256 received);

    constructor(IBrokerRegistry knownRegistry,address publishedHub) {
        require(address(knownRegistry).code.length != 0 && publishedHub.code.length != 0, "roles");
        registry=knownRegistry;
        issuerHub=publishedHub;
    }

    function execute(Order calldata order) external returns(uint256) {
        return settle(order,msg.sender);
    }

    function swap(IBalancerVault.SingleSwap calldata leg,IBalancerVault.FundManagement calldata funds,
                  uint256 minimum,uint256 deadline) external returns(uint256) {
        require(leg.kind == 0 && leg.userData.length == 0 && uint256(leg.poolId) >> 160 == 0 &&
                !funds.fromInternalBalance && !funds.toInternalBalance && funds.sender == msg.sender, "step");
        return settle(Order(address(uint160(uint256(leg.poolId))),leg.assetIn,leg.assetOut,
                            leg.amount,minimum,deadline),funds.recipient);
    }

    function tokenCall(address token,bytes memory data) private {
        (bool ok,bytes memory value)=token.call(data);
        require(ok && token.code.length != 0 && (value.length == 0 || abi.decode(value,(bool))), "token");
    }

    function settle(Order memory o,address recipient) private returns(uint256 received) {
        require(!entered && block.timestamp <= o.deadline && o.budget != 0 && o.minimum != 0 &&
                recipient != address(0) && recipient != address(this) && o.source != o.target, "order");
        entered=true;
        IEquityBroker broker=IEquityBroker(o.broker);
        address cash=broker.base();
        address share=broker.token();
        bool buying=o.source == cash && o.target == share;
        require(buying || (o.source == share && o.target == cash), "assets");
        address hub=broker.paymenthub();
        // New issuer deployments can use the published hub without a legacy registry entry.
        // Their broker and tokens are explicitly bound in the order. Funds always belong to its signer.
        require(registry.getBrokerbot(cash,share) == o.broker || hub == issuerHub, "issuer");
        require((broker.settings() & (buying ? 1 : 2)) != 0, "direction");
        IERC20 source=IERC20(o.source);
        IERC20 target=IERC20(o.target);
        uint256 priorSource=source.balanceOf(address(this));
        uint256 priorTarget=target.balanceOf(address(this));
        uint256 payerBefore=source.balanceOf(msg.sender);
        uint256 recipientBefore=target.balanceOf(recipient);
        uint256 spent=o.budget;
        if (buying) {
            uint256 units=broker.getShares(o.budget);
            uint256 available=target.balanceOf(o.broker);
            if (units > available) units=available;
            require(units >= o.minimum && units > 0, "shares");
            spent=broker.getBuyPrice(units);
            require(spent > 0 && spent <= o.budget, "share cost");
        }
        tokenCall(o.source,abi.encodeCall(IERC20.transferFrom,(msg.sender,address(this),spent)));
        require(source.balanceOf(address(this)) - priorSource == spent, "funding");
        if (buying) {
            tokenCall(o.source,abi.encodeCall(IERC20.approve,(hub,spent)));
            ISharePayment(hub).payAndNotify(o.broker,spent,hex"01");
            if (source.allowance(address(this),hub) != 0) {
                tokenCall(o.source,abi.encodeCall(IERC20.approve,(hub,0)));
                require(source.allowance(address(this),hub) == 0, "allowance");
            }
        } else {
            tokenCall(o.source,abi.encodeCall(ICallbackShare.transferAndCall,(o.broker,spent,hex"01")));
        }
        uint256 proceeds=target.balanceOf(address(this)) - priorTarget;
        require(proceeds >= o.minimum, "broker output");
        tokenCall(o.target,abi.encodeCall(IERC20.transfer,(recipient,proceeds)));
        received=target.balanceOf(recipient) - recipientBefore;
        require(received >= o.minimum && target.balanceOf(address(this)) == priorTarget &&
                source.balanceOf(address(this)) == priorSource && payerBefore-source.balanceOf(msg.sender) == spent, "cash");
        emit Paid(msg.sender,recipient,o.broker,o.source,o.target,spent,received);
        entered=false;
    }
}
