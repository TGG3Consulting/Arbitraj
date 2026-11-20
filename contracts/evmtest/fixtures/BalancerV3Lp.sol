// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/BalancerV3Lp.sol
// Что делает: ФИКСТУРА для EVM-теста шага BALANCER_V3 (решение 458, T457-01). Само хранилище — официальный код
//   (vendor/balancer-deployments, артефакты развёрнутого Vault V3 20241204); здесь только:
//   1) init — первая ликвидность пула: unlock → initialize(пул, себе, токены, суммы, 0, «») → перевод каждой суммы
//      хранилищу + settle (как Router.initializeHook официальной периферии: _takeTokenIn — перевод и settle);
//   2) quoteIn — котировка точного входа официальным путём хранилища: quote(данные) (VaultExtension.sol L869-L872,
//      только при tx.origin = 0 — EVMCallModeHelpers.isStaticCall) → хранилище зовёт у этого контракта qHook →
//      swap(EXACT_IN, пул, вход, выход, сумма, предел 0, «») → выход (как Router.querySwapHook). Состояние не меняется
//      (вызов с origin 0 тест делает в checkpoint и откатывает).
// Кто вызывает: contracts/evmtest/balancerv3.mjs.
// От чего зависит: подписи — @balancer-labs/v3-interfaces IVaultMain (unlock, settle, swap), IVaultExtension
//   (initialize, quote) из build-info 20241204-v3-vault (balancer/balancer-deployments@b55dc2b8).
// Настройки: нет.
pragma solidity 0.8.37;

struct BV3SwapParams {
    uint8 kind;
    address pool;
    address tokenIn;
    address tokenOut;
    uint256 amountGivenRaw;
    uint256 limitRaw;
    bytes userData;
}

interface IBV3Vault {
    function unlock(bytes calldata data) external returns (bytes memory);
    function initialize(address pool, address to, address[] memory tokens, uint256[] memory exactAmountsIn,
        uint256 minBptAmountOut, bytes memory userData) external returns (uint256);
    function settle(address token, uint256 amountHint) external returns (uint256);
    function swap(BV3SwapParams memory p) external returns (uint256, uint256, uint256);
    function quote(bytes calldata data) external returns (bytes memory);
}

interface IBV3Token {
    function transfer(address to, uint256 amount) external returns (bool);
}

contract BalancerV3Lp {
    // Первая ликвидность пула. Токены — на балансе этого контракта. Ошибки: откат хранилища.
    function init(address vault, address pool, address[] calldata tokens, uint256[] calldata amounts) external {
        IBV3Vault(vault).unlock(abi.encodeCall(this.initHook, (vault, pool, tokens, amounts)));
    }

    function initHook(address vault, address pool, address[] calldata tokens, uint256[] calldata amounts) external {
        require(msg.sender == vault, "vault");
        IBV3Vault(vault).initialize(pool, address(this), tokens, amounts, 0, "");
        for (uint256 i = 0; i < tokens.length; ++i) {
            require(IBV3Token(tokens[i]).transfer(vault, amounts[i]), "transfer");
            IBV3Vault(vault).settle(tokens[i], amounts[i]);
        }
    }

    // Котировка точного входа хранилищем (quote → qHook → swap). Возвращает: выход.
    function quoteIn(address vault, address pool, address tokenIn, address tokenOut, uint256 amount)
        external
        returns (uint256)
    {
        bytes memory r = IBV3Vault(vault).quote(abi.encodeCall(this.qHook, (vault, pool, tokenIn, tokenOut, amount)));
        return abi.decode(r, (uint256));
    }

    function qHook(address vault, address pool, address tokenIn, address tokenOut, uint256 amount)
        external
        returns (uint256 out)
    {
        require(msg.sender == vault, "vault");
        (,, out) = IBV3Vault(vault).swap(BV3SwapParams(0, pool, tokenIn, tokenOut, amount, 0, ""));
    }
}
