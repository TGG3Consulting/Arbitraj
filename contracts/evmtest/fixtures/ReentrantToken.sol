// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/ReentrantToken.sol
// Что делает: ТЕСТОВЫЙ токен (фикстура) для EVM-теста ArbExecutor (решение 272): ERC-20, который в transfer/transferFrom
//   пытается повторно войти в исполнитель (executeNative и execute) — проверка, что повторный вход откатывается.
//   В сети не разворачивается.
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

contract ReentrantToken {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;
    address public target;
    bytes public reenter;

    constructor(uint256 supply) {
        balanceOf[msg.sender] = supply;
    }

    function arm(address t, bytes calldata data) external {
        target = t;
        reenter = data;
    }

    function approve(address s, uint256 v) external returns (bool) {
        allowance[msg.sender][s] = v;
        return true;
    }

    function transfer(address to, uint256 v) external returns (bool) {
        _move(msg.sender, to, v);
        return true;
    }

    function transferFrom(address f, address to, uint256 v) external returns (bool) {
        allowance[f][msg.sender] -= v;
        _move(f, to, v);
        return true;
    }

    function _move(address f, address to, uint256 v) internal {
        balanceOf[f] -= v;
        balanceOf[to] += v;
        if (target != address(0)) {
            (bool ok, bytes memory ret) = target.call(reenter);
            if (!ok) {
                assembly {
                    revert(add(ret, 32), mload(ret))
                }
            }
        }
    }
}
