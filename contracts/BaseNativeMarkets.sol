// SPDX-License-Identifier: MIT
// Единый модуль для оплаты из WETH на нативных рынках o1 Standard и Omni Flux в Base.
// Владелец размещает модуль со своими проверенными адресами. В каждой заявке EOA или V5
// подписывает точный PoolId. Сумма входа, минимум и остатки проверяются внутри платежа.
pragma solidity 0.8.37;

import {BaseNativeSettlement, IO1Wrapped, IO1Asset} from "./BaseNativeSettlement.sol";
import {IV4PoolManager, ISingletonSettle, IInfVault, IInfClPoolManager, IInfBinPoolManager, InfClSwapParams, InfPoolKey} from "./IPools.sol";

interface IFluxDirectory {
    function vault() external view returns (IInfVault);
    function poolIdToPoolKey(bytes32 id) external view returns (InfPoolKey memory);
}

contract BaseNativeMarkets is BaseNativeSettlement {
    bool transient private nativeOrder;
    IInfVault public immutable fluxVault;
    IFluxDirectory public immutable fluxCl;
    IFluxDirectory public immutable fluxBin;

    constructor(IV4PoolManager poolManager, IO1Wrapped weth, address current, address a, address b,
                IInfVault vault, IFluxDirectory cl, IFluxDirectory bin)
        BaseNativeSettlement(poolManager, weth, current, a, b)
    {
        require(address(vault).code.length != 0 && address(cl) != address(bin) &&
                cl.vault() == vault && bin.vault() == vault, "flux");
        fluxVault = vault;
        fluxCl = cl;
        fluxBin = bin;
    }

    function fluxMarket(bytes32 id, address token) public view returns (InfPoolKey memory key) {
        require(token != address(wrapped) && token.code.length > 0, "asset");
        key = fluxCl.poolIdToPoolKey(id);
        if (key.poolManager != address(fluxCl)) {
            key = fluxBin.poolIdToPoolKey(id);
            require(key.poolManager == address(fluxBin), "manager");
        }
        require(key.currency0 == address(0) && key.currency1 == token && key.hooks == address(0) &&
                uint16(uint256(key.parameters)) == 0 && keccak256(abi.encode(key)) == id, "key");
    }

    function _executeMarket(bytes32 id, address token, bool buy, uint256 qty, address recipient) internal override {
        address[3] memory hooks = [currentHook, legacyHookA, legacyHookB];
        for (uint256 i; i < hooks.length; ++i) {
            if (keccak256(abi.encode(address(0), token, uint24(0), int24(200), hooks[i])) == id) {
                super._executeMarket(id, token, buy, qty, recipient);
                return;
            }
        }
        _executeFlux(id, token, buy, qty, recipient);
    }

    function _executeFlux(bytes32 id, address token, bool buy, uint256 qty, address recipient) private {
        require(recipient != address(fluxVault) && recipient != address(fluxCl) && recipient != address(fluxBin), "recipient");
        InfPoolKey memory key = fluxMarket(id, token);
        bytes memory data = abi.encode(key, buy, qty, recipient);
        expected = keccak256(data);
        fluxVault.lock(data);
    }

    function lockAcquired(bytes calldata data) external returns (bytes memory) {
        require(msg.sender == address(fluxVault) && entered && expected == keccak256(data), "callback");
        expected = 0;
        (InfPoolKey memory key, bool buy, uint256 qty, address recipient) = abi.decode(data, (InfPoolKey, bool, uint256, address));
        if (key.poolManager == address(fluxCl)) {
            IInfClPoolManager(key.poolManager).swap(key, InfClSwapParams(buy, -int256(qty),
                buy ? 4295128740 : 1461446703485210103287273052203988822378723970341), "");
        } else {
            require(key.poolManager == address(fluxBin), "manager");
            IInfBinPoolManager(key.poolManager).swap(key, buy, -int128(int256(qty)), "");
        }
        address input = buy ? address(0) : key.currency1;
        address output = buy ? key.currency1 : address(0);
        _payMarket(address(fluxVault), input, output, qty, recipient,
            fluxVault.currencyDelta(address(this), input), fluxVault.currencyDelta(address(this), output));
        return "";
    }

    /// Физический ETH с кошелька. Получатель равен плательщику. Обе стороны проверяются внутри одного вызова.
    function executeNative(bytes32 id, address token, bool buy, uint256 qty, uint256 minimum, uint256 deadline)
        external payable returns (uint256 net)
    {
        require(!entered && block.timestamp <= deadline && qty > 0 && qty <= uint256(uint128(type(int128).max)) &&
                minimum > 0 && msg.value == (buy ? qty : 0), "order");
        entered = true;
        nativeOrder = true;
        uint256 nativeStart = address(this).balance - msg.value;
        uint256 ethWallet = msg.sender.balance;
        uint256 tokenWallet = IO1Asset(token).balanceOf(msg.sender);
        uint256 own = IO1Asset(token).balanceOf(address(this));
        if (!buy) {
            _call(token, abi.encodeCall(IO1Asset.transferFrom, (msg.sender, address(this), qty)));
            require(IO1Asset(token).balanceOf(address(this)) - own == qty, "funding");
        }
        _executeFlux(id, token, buy, qty, msg.sender);
        require(expected == 0 && IO1Asset(token).balanceOf(address(this)) == own &&
                address(this).balance == nativeStart, "residue");
        if (buy) {
            require(msg.sender.balance == ethWallet, "payer");
            net = IO1Asset(token).balanceOf(msg.sender) - tokenWallet;
        } else {
            require(tokenWallet - IO1Asset(token).balanceOf(msg.sender) == qty, "payer");
            net = msg.sender.balance - ethWallet;
        }
        require(net >= minimum, "minimum");
        emit Filled(msg.sender, msg.sender, buy ? address(0) : token, buy ? token : address(0), qty, net, id);
        nativeOrder = false;
        entered = false;
    }

    function _payMarket(address locker, address input, address output, uint256 qty, address recipient,
                        int256 debt, int256 credit) internal override {
        if (!nativeOrder) {
            super._payMarket(locker, input, output, qty, recipient, debt, credit);
            return;
        }
        require(locker == address(fluxVault) && debt == -int256(qty) && credit > 0, "partial");
        ISingletonSettle pay = ISingletonSettle(locker);
        if (input == address(0)) {
            require(pay.settle{value: qty}() == qty, "settle ETH");
        } else {
            pay.sync(input);
            _call(input, abi.encodeCall(IO1Asset.transfer, (locker, qty)));
            require(pay.settle() == qty, "settle token");
        }
        pay.take(output, recipient, uint256(credit));
    }

    function _nativeSource(address sender) internal view override returns (bool) {
        return sender == address(fluxVault) || super._nativeSource(sender);
    }
}
