// SPDX-License-Identifier: MIT
// Общий платёж из WETH для нативных рынков Base. Исторический O1NativeAdapter сохранён отдельным исходником для воспроизведения байткода архивной симуляции.
// Модуль размещает владелец. В обоих способах оплаты подписывается точный PoolId.
// Ключ восстанавливается по зарегистрированному хуку. Дельты менеджера
// включают удержание хука. Остатки ETH и токенов модуля сохраняются.
pragma solidity 0.8.37;

import {IV4PoolManager, ISingletonSettle, V4PoolKey, V4SwapParams, IBalancerVault} from "./IPools.sol";

interface IO1Asset {
    function balanceOf(address owner) external view returns (uint256);
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

interface IO1Wrapped is IO1Asset {
    function deposit() external payable;
    function withdraw(uint256 amount) external;
}

abstract contract BaseNativeSettlement {
    IV4PoolManager public immutable manager;
    IO1Wrapped public immutable wrapped;
    address public immutable currentHook;
    address public immutable legacyHookA;
    address public immutable legacyHookB;
    bytes32 transient internal expected;
    bool transient internal entered;

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

    function market(bytes32 id, address token) public view returns (V4PoolKey memory) {
        require(token != address(wrapped) && token.code.length > 0, "asset");
        address[3] memory hooks = [currentHook, legacyHookA, legacyHookB];
        for (uint256 i; i < hooks.length; ++i) {
            V4PoolKey memory key = V4PoolKey(address(0), token, 0, 200, hooks[i]);
            if (keccak256(abi.encode(key)) != id) continue;
            (bool ok, bytes memory config) = hooks[i].staticcall(
                abi.encodeWithSignature("poolConfig(bytes32)", keccak256(abi.encode(key))));
            require(ok && config.length == (i == 0 ? 256 : 352), "config");
            (bool initialized, bool tokenIsCurrency0) = abi.decode(config, (bool, bool));
            require(initialized && !tokenIsCurrency0, "unlisted");
            return key;
        }
        revert("unknown key");
    }

    function execute(bytes32 id, address tokenIn, address tokenOut, uint256 qty, uint256 minimum,
                     uint256 deadline) external returns (uint256) {
        return _settle(id, tokenIn, tokenOut, qty, minimum, msg.sender, deadline);
    }

    function swap(IBalancerVault.SingleSwap calldata order, IBalancerVault.FundManagement calldata funds,
                  uint256 minimum, uint256 deadline) external payable returns (uint256) {
        require(msg.value == 0 && order.kind == 0 && order.userData.length == 0 && funds.sender == msg.sender &&
                !funds.fromInternalBalance && !funds.toInternalBalance, "order");
        return _settle(order.poolId, order.assetIn, order.assetOut, order.amount, minimum, funds.recipient, deadline);
    }

    function _settle(bytes32 id, address tokenIn, address tokenOut, uint256 qty, uint256 minimum,
                     address recipient, uint256 deadline) private returns (uint256 received) {
        require(!entered && block.timestamp <= deadline && qty > 0 && qty <= uint256(uint128(type(int128).max)), "order");
        require((tokenIn == address(wrapped)) != (tokenOut == address(wrapped)), "currencies");
        require(recipient != address(0) && recipient != address(this) && recipient != address(manager), "recipient");
        entered = true;
        bool buy = tokenIn == address(wrapped);
        uint256 nativeBefore = address(this).balance;
        uint256 ownIn = IO1Asset(tokenIn).balanceOf(address(this));
        uint256 ownOut = IO1Asset(tokenOut).balanceOf(address(this));
        uint256 payerBefore = IO1Asset(tokenIn).balanceOf(msg.sender);
        uint256 recipientBefore = IO1Asset(tokenOut).balanceOf(recipient);
        _call(tokenIn, abi.encodeCall(IO1Asset.transferFrom, (msg.sender, address(this), qty)));
        require(IO1Asset(tokenIn).balanceOf(address(this)) - ownIn == qty, "funding");
        _executeMarket(id, buy ? tokenOut : tokenIn, buy, qty, recipient);
        require(expected == bytes32(0) && payerBefore - IO1Asset(tokenIn).balanceOf(msg.sender) == qty, "payer");
        require(address(this).balance == nativeBefore && IO1Asset(tokenIn).balanceOf(address(this)) == ownIn &&
                IO1Asset(tokenOut).balanceOf(address(this)) == ownOut, "residue");
        received = IO1Asset(tokenOut).balanceOf(recipient) - recipientBefore;
        require(received > 0 && received >= minimum, "minimum");
        emit Filled(msg.sender, recipient, tokenIn, tokenOut, qty, received, id);
        entered = false;
    }

    function _executeMarket(bytes32 id, address token, bool buy, uint256 qty, address recipient) internal virtual {
        V4PoolKey memory key = market(id, token);
        bytes memory data = abi.encode(key, buy, qty, recipient);
        expected = keccak256(data);
        manager.unlock(data);
    }

    function unlockCallback(bytes calldata data) external returns (bytes memory) {
        require(msg.sender == address(manager) && entered && expected == keccak256(data), "callback");
        expected = 0;
        (V4PoolKey memory key, bool buy, uint256 qty, address recipient) =
            abi.decode(data, (V4PoolKey, bool, uint256, address));
        manager.swap(key, V4SwapParams(buy, -int256(qty), buy ? 4295128740 : 1461446703485210103287273052203988822378723970341), "");
        address input = buy ? address(0) : key.currency1;
        address output = buy ? key.currency1 : address(0);
        _payMarket(address(manager), input, output, qty, recipient, _delta(input), _delta(output));
        return "";
    }

    function _payMarket(address locker, address input, address output, uint256 qty, address recipient,
                        int256 debt, int256 credit) internal {
        require(debt == -int256(qty), "partial");
        require(credit > 0, "output");
        ISingletonSettle pay = ISingletonSettle(locker);
        if (input == address(0)) {
            wrapped.withdraw(qty);
            require(pay.settle{value: qty}() == qty, "settle ETH");
            pay.take(output, recipient, uint256(credit));
        } else {
            pay.sync(input);
            _call(input, abi.encodeCall(IO1Asset.transfer, (locker, qty)));
            require(pay.settle() == qty, "settle token");
            pay.take(address(0), address(this), uint256(credit));
            wrapped.deposit{value: uint256(credit)}();
            _call(address(wrapped), abi.encodeCall(IO1Asset.transfer, (recipient, uint256(credit))));
        }
    }

    function _delta(address currency) private view returns (int256) {
        return int256(uint256(manager.exttload(keccak256(abi.encode(address(this), currency)))));
    }

    function _call(address token, bytes memory payload) private {
        (bool ok, bytes memory data) = token.call(payload);
        require(ok && (data.length == 0 || abi.decode(data, (bool))), "transfer");
    }

    function _nativeSource(address sender) internal view virtual returns (bool) {
        return sender == address(wrapped) || sender == address(manager);
    }

    receive() external payable {
        require(entered && _nativeSource(msg.sender), "source");
    }
}
