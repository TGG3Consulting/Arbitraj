// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

interface IIntegralToken {
    function balanceOf(address who) external view returns (uint256);
    function allowance(address who, address spender) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}
interface IIntegralFactory {
    function getPair(address a, address b) external view returns (address);
}
interface IIntegralRoles {
    function factory() external view returns (address);
    function relayer() external view returns (address);
    function trader() external view returns (address);
}
interface IIntegralRelay {
    struct Order {
        address input;
        address output;
        uint256 inputLimit;
        uint256 outputLimit;
        bool wrapUnwrap;
        address recipient;
        uint32 deadline;
    }
    function implementation() external view returns (address);
    function delay() external view returns (address);
    function factory() external view returns (address);
    function isPairEnabled(address pair) external view returns (bool);
    function quoteBuy(address a, address b, uint256 amountOut) external view returns (uint256);
    function sell(Order calldata order) external returns (uint256);
    function buy(Order calldata order) external returns (uint256);
}

/// Instant payment through Integral SIZE's relayer. Its later inventory order belongs to the relayer.
contract IntegralSizeAdapter {
    IIntegralRelay public immutable relayer;
    address public immutable implementation;
    IIntegralFactory public immutable factory;
    address public immutable delay;
    bool transient private entered;

    error InvalidMarket();
    error InvalidOrder();
    error Payment();
    event Filled(address indexed payer, address indexed recipient, address indexed pair,
        address input, address output, uint256 spent, uint256 received, uint256 orderId, bool exactOutput);

    constructor(IIntegralRelay proxy, address acceptedImplementation, IIntegralFactory registry, address queue) {
        if (acceptedImplementation.code.length == 0 || address(registry).code.length == 0 ||
            proxy.implementation() != acceptedImplementation || proxy.factory() != address(registry) ||
            proxy.delay() != queue || IIntegralRoles(queue).factory() != address(registry) ||
            IIntegralRoles(queue).relayer() != address(proxy)) revert InvalidMarket();
        relayer = proxy;
        implementation = acceptedImplementation;
        factory = registry;
        delay = queue;
    }

    function swapFrom(address input, address output, uint256 amount, uint256 minimum,
        address recipient, uint256 deadline) external returns (uint256 received) {
        (,received) = settle(input, output, amount, minimum, recipient, deadline, false);
    }

    function buy(address input, address output, uint256 maxInput, uint256 targetOut,
        address recipient, uint256 deadline) external returns (uint256 spent, uint256 received) {
        return settle(input, output, maxInput, targetOut, recipient, deadline, true);
    }

    function market(address input, address output) private view returns (address pair) {
        if (relayer.implementation() != implementation || relayer.factory() != address(factory) ||
            relayer.delay() != delay || IIntegralRoles(delay).relayer() != address(relayer)) revert InvalidMarket();
        pair = factory.getPair(input, output);
        if (pair == address(0) || IIntegralRoles(pair).factory() != address(factory) ||
            IIntegralRoles(pair).trader() != delay || !relayer.isPairEnabled(pair)) revert InvalidMarket();
    }

    function move(address token, bytes memory data) private {
        (bool ok, bytes memory returned) = token.call(data);
        if (!ok || (returned.length != 0 && !abi.decode(returned, (bool)))) revert Payment();
    }

    function settle(address input, address output, uint256 maxInput, uint256 minimum, address recipient,
        uint256 deadline, bool exactOutput) private returns (uint256 spent, uint256 received) {
        if (entered || input == output || recipient == address(0) || recipient == address(this) ||
            maxInput == 0 || (exactOutput && minimum == 0) || deadline < block.timestamp ||
            deadline > type(uint32).max) revert InvalidOrder();
        entered = true;
        address pair = market(input, output);
        IIntegralToken coin = IIntegralToken(input);
        uint256 initialPayer = coin.balanceOf(msg.sender);
        uint256 initialAsset = coin.balanceOf(address(this));
        uint256 initialOut = IIntegralToken(output).balanceOf(recipient);
        uint256 amount = exactOutput ? relayer.quoteBuy(input, output, minimum) : maxInput;
        if (amount == 0 || amount > maxInput) revert Payment();
        move(input, abi.encodeCall(coin.transferFrom, (msg.sender, address(this), amount)));
        uint256 funded = coin.balanceOf(address(this)) - initialAsset;
        if (funded == 0 || funded > amount || (exactOutput && funded != amount)) revert Payment();
        move(input, abi.encodeCall(coin.approve, (address(relayer), funded)));
        IIntegralRelay.Order memory order = IIntegralRelay.Order(
            input, output, funded, minimum, false, recipient, uint32(deadline));
        uint256 orderId = exactOutput ? relayer.buy(order) : relayer.sell(order);
        if (coin.allowance(address(this), address(relayer)) != 0) {
            move(input, abi.encodeCall(coin.approve, (address(relayer), 0)));
        }
        spent = initialPayer - coin.balanceOf(msg.sender);
        received = IIntegralToken(output).balanceOf(recipient) - initialOut;
        if (coin.balanceOf(address(this)) != initialAsset || coin.allowance(address(this),address(relayer)) != 0 ||
            orderId == 0 || spent == 0 || spent > maxInput || (!exactOutput && spent != maxInput) ||
            received == 0 || received < minimum) revert Payment();
        emit Filled(msg.sender, recipient, pair, input, output, spent, received, orderId, exactOutput);
        entered = false;
    }
}
