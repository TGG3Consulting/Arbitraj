// SPDX-License-Identifier: MIT
// Единый модуль для оплаты из WETH на нативных рынках o1 Standard и Omni Flux в Base.
// Владелец размещает модуль со своими проверенными адресами. В каждой заявке EOA или V5
// подписывает точный PoolId. Сумма входа, минимум и остатки проверяются внутри платежа.
pragma solidity 0.8.37;

import {BaseNativeSettlement, IO1Wrapped} from "./BaseNativeSettlement.sol";
import {IV4PoolManager, IInfVault, IInfClPoolManager, IInfBinPoolManager, InfClSwapParams, InfPoolKey} from "./IPools.sol";

interface IFluxDirectory {
    function vault() external view returns (IInfVault);
    function poolIdToPoolKey(bytes32 id) external view returns (InfPoolKey memory);
}

contract BaseNativeMarkets is BaseNativeSettlement {
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

    function _nativeSource(address sender) internal view override returns (bool) {
        return sender == address(fluxVault) || super._nativeSource(sender);
    }
}
