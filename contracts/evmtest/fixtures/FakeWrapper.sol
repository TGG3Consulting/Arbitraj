// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/FakeWrapper.sol
// Что делает: ТЕСТОВАЯ поддельная обёртка (фикстура) для EVM-теста ArbExecutor (ревью 272, А4): ERC-20 с deposit
//   (принимает BNB и начисляет токен), но withdraw только списывает токен и BNB НЕ отдаёт. Проверка: исполнитель
//   считает «получено» и событие по BNB, реально пришедшему при withdraw, а не по сумме withdraw. В сети не
//   разворачивается.
// Кто вызывает: contracts/evmtest/review.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

contract FakeWrapper {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function deposit() external payable {
        balanceOf[msg.sender] += msg.value;
    }

    function withdraw(uint256 v) external {
        balanceOf[msg.sender] -= v;
    }

    function approve(address s, uint256 v) external returns (bool) {
        allowance[msg.sender][s] = v;
        return true;
    }

    function transfer(address to, uint256 v) external returns (bool) {
        balanceOf[msg.sender] -= v;
        balanceOf[to] += v;
        return true;
    }

    function transferFrom(address f, address to, uint256 v) external returns (bool) {
        allowance[f][msg.sender] -= v;
        balanceOf[f] -= v;
        balanceOf[to] += v;
        return true;
    }
}
