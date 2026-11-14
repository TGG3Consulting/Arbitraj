// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/OwnerProxy.sol
// Что делает: ТЕСТОВЫЙ контракт-владелец (фикстура) для EVM-теста ArbExecutor (ревью 272, А4): разворачивает исполнитель
//   от своего имени (владелец исполнителя = этот контракт) и пересылает любой вызов исполнителю от имени владельца.
//   Нужен, чтобы повторный вход пришёл ИМЕННО от владельца (кошелёк бота — EOA, изнутри транзакции он не вызывает):
//   так проверяется замок locked, а не require(owner). Принимает BNB (владелец получает остаток / покупку). В сети не
//   разворачивается.
// Кто вызывает: contracts/evmtest/review.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

contract OwnerProxy {
    // Разворачивает контракт кодом создания init (с аргументами конструктора). Возвращает адрес.
    function deploy(bytes memory init) external returns (address a) {
        assembly {
            a := create(0, add(init, 32), mload(init))
        }
        require(a != address(0), "create");
    }

    // Вызов to с данными data и BNB от имени этого контракта; откат — с той же причиной.
    function run(address to, bytes calldata data) external payable returns (bytes memory) {
        (bool ok, bytes memory ret) = to.call{value: msg.value}(data);
        if (!ok) {
            assembly {
                revert(add(ret, 32), mload(ret))
            }
        }
        return ret;
    }

    receive() external payable {}
}
