// SPDX-License-Identifier: MIT
// Владелец размещает этот модуль для одной проверенной CurveFactory Stabull.
// Кошелёк вызывает execute или executeTarget, ArbExecutor V5 передаёт адрес Curve в шаге 17.
// После обмена модуль сверяет чистую выплату, расход плательщика и возврат остатка по заданному лимиту.
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface IStabullFactory {
    function getCurve(address base, address quote) external view returns (address);
    function getGlobalFrozenState() external view returns (bool);
}
interface IStabullPool {
    function derivatives(uint256 index) external view returns (address);
    function emergency() external view returns (bool);
    function frozen() external view returns (bool);
    function originSwap(address input,address output,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);
    function targetSwap(address input,address output,uint256 maximum,uint256 want,uint256 deadline) external returns(uint256);
}
interface IStabullAsset {
    function balanceOf(address account) external view returns(uint256);
    function allowance(address account,address delegate) external view returns(uint256);
    function approve(address delegate,uint256 amount) external returns(bool);
    function transfer(address to,uint256 amount) external returns(bool);
    function transferFrom(address from,address to,uint256 amount) external returns(bool);
}

contract StabullAdapter {
    IStabullFactory public immutable factory;
    bool transient private entered;

    error InvalidMarket();
    error InvalidOrder();
    error Payment();
    event Filled(address indexed payer,address indexed recipient,address indexed pool,
                 address input,address output,uint256 paid,uint256 received);

    constructor(IStabullFactory factory_) {
        if (address(factory_).code.length == 0) revert InvalidMarket();
        factory = factory_;
    }

    function execute(address pool,address input,address output,uint256 amount,uint256 minimum,uint256 deadline)
        external returns(uint256 out) {
        (,out) = settle(pool,input,output,amount,minimum,deadline,msg.sender,false);
    }

    function executeTarget(address pool,address input,address output,uint256 maximum,uint256 want,uint256 deadline)
        external returns(uint256 paid,uint256 received) {
        return settle(pool,input,output,maximum,want,deadline,msg.sender,true);
    }

    function swap(IBalancerVault.SingleSwap calldata o, IBalancerVault.FundManagement calldata f,
                  uint256 minimum,uint256 deadline) external returns(uint256 received) {
        if (o.kind != 0 || o.userData.length != 0 || f.sender != msg.sender || f.fromInternalBalance ||
            f.toInternalBalance || uint256(o.poolId) > type(uint160).max) revert InvalidOrder();
        // V5 проверяет собственный deadline, затем передаёт этому шагу ровно block.timestamp.
        // Curve требует строгого неравенства. Разрешение действует только внутри текущей транзакции.
        if (deadline < block.timestamp) revert InvalidOrder();
        uint256 curveDeadline = deadline == block.timestamp ? deadline + 1 : deadline;
        (,received) = settle(address(uint160(uint256(o.poolId))),o.assetIn,o.assetOut,o.amount,minimum,curveDeadline,f.recipient,false);
    }

    function settle(address pool,address input,address output,uint256 budget,uint256 minimum,uint256 deadline,
                    address recipient,bool wanted) private returns(uint256 paid,uint256 received) {
        if (entered || budget == 0 || minimum == 0 || block.timestamp >= deadline || input == output ||
            input == address(0) || output == address(0) || recipient == pool || recipient == address(this) ||
            recipient == address(0) || msg.sender == pool) revert InvalidOrder();
        entered = true;
        IStabullPool curve = IStabullPool(pool);
        address base = curve.derivatives(0);
        address quote = curve.derivatives(1);
        if (!((input == base && output == quote) || (input == quote && output == base)) ||
            factory.getCurve(base,quote) != pool || factory.getGlobalFrozenState() ||
            curve.frozen() || curve.emergency()) revert InvalidMarket();

        IStabullAsset src = IStabullAsset(input);
        IStabullAsset dst = IStabullAsset(output);
        uint256 ownerIn = src.balanceOf(msg.sender);
        uint256 ownerOut = dst.balanceOf(recipient);
        uint256 heldIn = src.balanceOf(address(this));
        uint256 heldOut = dst.balanceOf(address(this));

        if (src.allowance(address(this),pool) != 0) revert Payment();
        tokenCall(src,abi.encodeCall(src.transferFrom,(msg.sender,address(this),budget)));
        if (src.balanceOf(address(this)) != heldIn + budget) revert Payment();
        tokenCall(src,abi.encodeCall(src.approve,(pool,budget)));

        uint256 nominal = wanted ? curve.targetSwap(input,output,budget,minimum,deadline)
                                 : curve.originSwap(input,output,budget,minimum,deadline);
        uint256 rest = src.balanceOf(address(this)) - heldIn;
        paid = budget - rest;
        if (paid == 0 || (wanted ? paid != nominal : rest != 0)) revert Payment();
        if (src.allowance(address(this),pool) != 0) tokenCall(src,abi.encodeCall(src.approve,(pool,0)));
        if (rest != 0) tokenCall(src,abi.encodeCall(src.transfer,(msg.sender,rest)));
        uint256 transferable = dst.balanceOf(address(this)) - heldOut;
        if (transferable < minimum) revert Payment();
        tokenCall(dst,abi.encodeCall(dst.transfer,(recipient,transferable)));
        received = dst.balanceOf(recipient) - ownerOut;
        if (received < minimum || ownerIn - src.balanceOf(msg.sender) != paid ||
            src.balanceOf(address(this)) != heldIn || dst.balanceOf(address(this)) != heldOut ||
            src.allowance(address(this),pool) != 0) revert Payment();
        emit Filled(msg.sender,recipient,pool,input,output,paid,received);
        entered = false;
    }

    function tokenCall(IStabullAsset token,bytes memory data) private {
        (bool ok,bytes memory ret) = address(token).call(data);
        if (!ok || (ret.length != 0 && !abi.decode(ret,(bool)))) revert Payment();
    }
}
