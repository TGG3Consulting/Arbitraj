// SPDX-License-Identifier: MIT
// Оплата собственного WETH на нативных рынках o1 Standard в Base.
// Ключ восстанавливается по зарегистрированному хуку. Дельты менеджера
// включают удержание хука. Остатки ETH и токенов модуля сохраняются.
pragma solidity 0.8.37;

import {IV4PoolManager, ISingletonSettle, V4PoolKey, V4SwapParams} from "./IPools.sol";

interface IO1Asset {
    function balanceOf(address owner) external view returns (uint256);
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

interface IO1Wrapped is IO1Asset {
    function deposit() external payable;
    function withdraw(uint256 amount) external;
}

contract O1NativeAdapter {
    IV4PoolManager public immutable manager;
    IO1Wrapped public immutable wrapped;
    address public immutable currentHook;
    address public immutable legacyHookA;
    address public immutable legacyHookB;
    bytes32 transient private expected;
    bool transient private entered;

    event Filled(address indexed payer, address indexed recipient, address indexed tokenIn,
                 address tokenOut, uint256 paid, uint256 received, bytes32 poolId);

    constructor(IV4PoolManager poolManager, IO1Wrapped weth, address current, address a, address b) {
        require(block.chainid == 8453 && address(poolManager).code.length > 0 && address(weth).code.length > 0, "network");
        require(current.code.length > 0 && a.code.length > 0 && b.code.length > 0 && current != a && current != b && a != b, "hooks");
        manager = poolManager;
        wrapped = weth;
        currentHook = current;
        legacyHookA = a;
        legacyHookB = b;
    }

    function market(address token) public view returns (V4PoolKey memory selected) {
        require(token != address(wrapped) && token.code.length > 0, "asset");
        address[3] memory hooks = [currentHook, legacyHookA, legacyHookB];
        for (uint256 i; i < hooks.length; ++i) {
            V4PoolKey memory key = V4PoolKey(address(0), token, 0, 200, hooks[i]);
            (bool ok, bytes memory config) = hooks[i].staticcall(
                abi.encodeWithSignature("poolConfig(bytes32)", keccak256(abi.encode(key))));
            require(ok && config.length == (i == 0 ? 256 : 352), "config");
            (bool initialized, bool tokenIsCurrency0) = abi.decode(config, (bool, bool));
            if (initialized) {
                require(!tokenIsCurrency0 && selected.hooks == address(0), "ambiguous");
                selected = key;
            }
        }
        require(selected.hooks != address(0), "unlisted");
    }

    function swapFrom(address tokenIn, address tokenOut, uint256 qty, uint256 minimum,
                      address recipient, uint256 deadline) external returns (uint256 received) {
        require(!entered && block.timestamp <= deadline && qty > 0 && qty <= uint256(uint128(type(int128).max)), "order");
        require((tokenIn == address(wrapped)) != (tokenOut == address(wrapped)), "currencies");
        require(recipient != address(0) && recipient != address(this) && recipient != address(manager), "recipient");
        entered = true;
        bool buy = tokenIn == address(wrapped);
        V4PoolKey memory key = market(buy ? tokenOut : tokenIn);
        uint256 nativeBefore = address(this).balance;
        uint256 ownIn = IO1Asset(tokenIn).balanceOf(address(this));
        uint256 ownOut = IO1Asset(tokenOut).balanceOf(address(this));
        uint256 payerBefore = IO1Asset(tokenIn).balanceOf(msg.sender);
        uint256 recipientBefore = IO1Asset(tokenOut).balanceOf(recipient);
        _call(tokenIn, abi.encodeCall(IO1Asset.transferFrom, (msg.sender, address(this), qty)));
        require(IO1Asset(tokenIn).balanceOf(address(this)) - ownIn == qty, "funding");
        bytes memory data = abi.encode(key, buy, qty, recipient);
        expected = keccak256(data);
        manager.unlock(data);
        require(expected == bytes32(0) && payerBefore - IO1Asset(tokenIn).balanceOf(msg.sender) == qty, "payer");
        require(address(this).balance == nativeBefore && IO1Asset(tokenIn).balanceOf(address(this)) == ownIn &&
                IO1Asset(tokenOut).balanceOf(address(this)) == ownOut, "residue");
        received = IO1Asset(tokenOut).balanceOf(recipient) - recipientBefore;
        require(received > 0 && received >= minimum, "minimum");
        emit Filled(msg.sender, recipient, tokenIn, tokenOut, qty, received, keccak256(abi.encode(key)));
        entered = false;
    }

    function unlockCallback(bytes calldata data) external returns (bytes memory) {
        require(msg.sender == address(manager) && entered && expected == keccak256(data), "callback");
        expected = 0;
        (V4PoolKey memory key, bool buy, uint256 qty, address recipient) =
            abi.decode(data, (V4PoolKey, bool, uint256, address));
        manager.swap(key, V4SwapParams(buy, -int256(qty), buy ? 4295128740 : 1461446703485210103287273052203988822378723970341), "");
        address input = buy ? address(0) : key.currency1;
        address output = buy ? key.currency1 : address(0);
        require(_delta(input) == -int256(qty), "partial");
        int256 credit = _delta(output);
        require(credit > 0, "output");
        ISingletonSettle pay = ISingletonSettle(address(manager));
        if (buy) {
            wrapped.withdraw(qty);
            require(pay.settle{value: qty}() == qty, "settle ETH");
            pay.take(output, recipient, uint256(credit));
        } else {
            pay.sync(input);
            _call(input, abi.encodeCall(IO1Asset.transfer, (address(manager), qty)));
            require(pay.settle() == qty, "settle token");
            pay.take(address(0), address(this), uint256(credit));
            wrapped.deposit{value: uint256(credit)}();
            _call(address(wrapped), abi.encodeCall(IO1Asset.transfer, (recipient, uint256(credit))));
        }
        return "";
    }

    function _delta(address currency) private view returns (int256) {
        return int256(uint256(manager.exttload(keccak256(abi.encode(address(this), currency)))));
    }

    function _call(address token, bytes memory payload) private {
        (bool ok, bytes memory data) = token.call(payload);
        require(ok && (data.length == 0 || abi.decode(data, (bool))), "transfer");
    }

    receive() external payable {
        require(entered && (msg.sender == address(wrapped) || msg.sender == address(manager)), "source");
    }
}
