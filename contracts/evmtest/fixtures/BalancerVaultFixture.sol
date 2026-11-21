// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/BalancerVaultFixture.sol
// Что делает: ТЕСТОВОЕ хранилище вида Balancer V2 Vault (фикстура) для EVM-теста шага BALANCER ArbExecutor v2 (решение 289):
//   официальный Vault (balancer-deployments 20210418-vault, solc 0.7.1) в репозиторий не скопирован — повторены только
//   подпись swap и строки обмена Swaps.sol L56-L104: вызывающий = funds.sender (authenticateFor без relayer), срок, сумма > 0,
//   токены разные, выход ≥ limit (GIVEN_IN), затем transferFrom(sender → хранилище, вход) (AssetTransfersHandler.sol L76) и
//   перевод выхода recipient. Зарегистрирован один id пула; чужой id — откат. Курс — 99 % от входа (формулы пулов проверяют
//   тесты бота на записанных ответах цепи). В сети не разворачивается.
// Кто вызывает: contracts/evmtest/balancer.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

interface ITokenB {
    function transfer(address to, uint256 v) external returns (bool);
    function transferFrom(address from, address to, uint256 v) external returns (bool);
}

contract BalancerVaultFixture {
    struct SingleSwap {
        bytes32 poolId;
        uint8 kind;
        address assetIn;
        address assetOut;
        uint256 amount;
        bytes userData;
    }
    struct FundManagement {
        address sender;
        bool fromInternalBalance;
        address payable recipient;
        bool toInternalBalance;
    }

    // Единственный зарегистрированный «пул»: id = адрес пула | специализация | номер (PoolRegistry.sol).
    bytes32 public poolId;

    constructor(bytes32 id) {
        poolId = id;
    }

    function swap(SingleSwap memory s, FundManagement memory f, uint256 limit, uint256 deadline)
        external
        payable
        returns (uint256 out)
    {
        require(msg.sender == f.sender, "BAL#401");
        require(block.timestamp <= deadline, "BAL#508");
        require(s.amount > 0, "BAL#510");
        require(s.assetIn != s.assetOut, "BAL#509");
        require(s.poolId == poolId, "BAL#500");
        require(s.kind == 0 && !f.fromInternalBalance && !f.toInternalBalance, "fixture: given in, no internal");
        out = s.amount * 99 / 100;
        require(out >= limit, "BAL#507");
        require(ITokenB(s.assetIn).transferFrom(f.sender, address(this), s.amount), "in");
        require(ITokenB(s.assetOut).transfer(f.recipient, out), "out");
    }
}
