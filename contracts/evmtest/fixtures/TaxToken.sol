// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/TaxToken.sol
// Что делает: ФИКСТУРА — ERC-20 с налогом на перевод 1 % (сгорает) для EVM-теста шагов-менеджеров (ревью t181 Н5):
//   менеджер засчитывает оплату по приросту своего баланса (settle), налог даёт недоплату → откат всего unlock / lock.
//   Освобождённые от налога адреса (exempt) — поставщик ликвидности и LP, чтобы пул можно было наполнить. В сети не
//   разворачивается.
// Кто вызывает: contracts/evmtest/singleton.mjs.
// От чего зависит: ничего (EIP-20: transfer / transferFrom / approve / balanceOf / allowance).
// Настройки: нет.
pragma solidity 0.8.37;

contract TaxToken {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;
    mapping(address => bool) public exempt;
    address public admin;

    constructor(uint256 supply) {
        admin = msg.sender;
        balanceOf[msg.sender] = supply;
        exempt[msg.sender] = true;
    }

    function setExempt(address a, bool e) external {
        require(msg.sender == admin, "admin");
        exempt[a] = e;
    }

    function approve(address s, uint256 v) external returns (bool) {
        allowance[msg.sender][s] = v;
        return true;
    }

    function transfer(address to, uint256 v) external returns (bool) {
        _move(msg.sender, to, v);
        return true;
    }

    function transferFrom(address from, address to, uint256 v) external returns (bool) {
        uint256 a = allowance[from][msg.sender];
        require(a >= v, "allowance");
        allowance[from][msg.sender] = a - v;
        _move(from, to, v);
        return true;
    }

    // Перевод: с отправителя — v, получателю — v без 1 % (если отправитель не освобождён).
    function _move(address from, address to, uint256 v) internal {
        require(balanceOf[from] >= v, "balance");
        balanceOf[from] -= v;
        balanceOf[to] += exempt[from] ? v : v - v / 100;
    }
}
