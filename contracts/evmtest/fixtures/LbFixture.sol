// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/LbFixture.sol
// Что делает: ТЕСТОВЫЙ пул вида Liquidity Book V2.1 (ФИКСТУРА) для EVM-теста шага LB ArbExecutor v2 (решение 290):
//   официальный LBPair (lfj-gg/joe-v2@v2.1.0, solc 0.8.10) в репозиторий не скопирован — повторены только интерфейс и
//   порядок обмена swap(bool swapForY, address to) (src/LBPair.sol L483-L557): вход = баланс пула − резерв (L487,
//   BinHelper.sol L290-L306), нулевой вход — откат (L488), выход — transfer `to` (L552-L556), резервы — после обмена;
//   getTokenX / getTokenY (L132-L142) — порядок X/Y задаёт создатель (как у клона), а не адреса. Кривая — курс 99 %
//   (бины и комиссию проверяют тесты бота на записанных ответах getSwapOut цепи). В сети не разворачивается.
// Кто вызывает: contracts/evmtest/lb.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

interface ITokenL {
    function transfer(address to, uint256 v) external returns (bool);
    function balanceOf(address a) external view returns (uint256);
}

contract LbFixture {
    address public immutable tokenX;
    address public immutable tokenY;
    uint256 internal reserveX;
    uint256 internal reserveY;

    // Принимает: токен X и токен Y в порядке создателя пула.
    constructor(address x, address y) {
        (tokenX, tokenY) = (x, y);
    }

    function getTokenX() external view returns (address) {
        return tokenX;
    }

    function getTokenY() external view returns (address) {
        return tokenY;
    }

    // Резервы = балансы (после пополнения поставщиком).
    function sync() external {
        reserveX = ITokenL(tokenX).balanceOf(address(this));
        reserveY = ITokenL(tokenY).balanceOf(address(this));
    }

    // Выход по курсу 99 % от входа = баланс − резерв; возвращает выход (в LBPair — упакованный bytes32).
    function swap(bool swapForY, address to) external returns (bytes32) {
        uint256 bx = ITokenL(tokenX).balanceOf(address(this));
        uint256 by = ITokenL(tokenY).balanceOf(address(this));
        uint256 got = swapForY ? bx - reserveX : by - reserveY;
        require(got > 0, "LBPair__InsufficientAmountIn");
        uint256 out = got * 99 / 100;
        require(out > 0, "LBPair__InsufficientAmountOut");
        ITokenL(swapForY ? tokenY : tokenX).transfer(to, out);
        reserveX = ITokenL(tokenX).balanceOf(address(this));
        reserveY = ITokenL(tokenY).balanceOf(address(this));
        return bytes32(out);
    }
}
