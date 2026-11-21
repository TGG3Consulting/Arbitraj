// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/IziFixture.sol
// Что делает: ТЕСТОВЫЙ пул вида iZiSwap (фикстура) для EVM-теста шага IZI ArbExecutor v2 (решение 286): повторены
//   подписи и порядок оплаты из izumiFinance/iZiSwap-core@3fe21eba (swapX2Y.sol L123-L131, L321-L328; swapY2X.sol
//   L290-L297; IiZiSwapCallback.sol L19-L39): swapX2Y(recipient, amount, lowPt, data) / swapY2X(recipient, amount,
//   highPt, data); выход переводится получателю, затем у вызывающего swapX2YCallback(x, y, data) (платить x) /
//   swapY2XCallback(x, y, data) (платить y); проверка «баланс входа вырос на долг» (XE / YE). Курс 1:1, но пул берёт
//   amount − 1 (настоящий пул берёт меньше на остаток, которого не хватает на 1 единицу выхода — tests/data/izi_2026-11-15/source_excerpt.txt). tokenX < tokenY. В сети не разворачивается.
// Кто вызывает: contracts/evmtest/izi.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

interface ITokenI {
    function transfer(address to, uint256 v) external returns (bool);
    function balanceOf(address a) external view returns (uint256);
}

interface IIziCallback {
    function swapX2YCallback(uint256 x, uint256 y, bytes calldata data) external;
    function swapY2XCallback(uint256 x, uint256 y, bytes calldata data) external;
}

contract IziFixture {
    address public tokenX;
    address public tokenY;

    constructor(address a, address b) {
        (tokenX, tokenY) = a < b ? (a, b) : (b, a);
    }

    function swapX2Y(address recipient, uint128 amount, int24, bytes calldata data)
        external
        returns (uint256 amountX, uint256 amountY)
    {
        amountX = uint256(amount) - 1;
        amountY = amountX;
        ITokenI(tokenY).transfer(recipient, amountY);
        uint256 bx = ITokenI(tokenX).balanceOf(address(this));
        IIziCallback(msg.sender).swapX2YCallback(amountX, amountY, data);
        require(ITokenI(tokenX).balanceOf(address(this)) >= bx + amountX, "XE");
    }

    function swapY2X(address recipient, uint128 amount, int24, bytes calldata data)
        external
        returns (uint256 amountX, uint256 amountY)
    {
        amountY = uint256(amount) - 1;
        amountX = amountY;
        ITokenI(tokenX).transfer(recipient, amountX);
        uint256 by = ITokenI(tokenY).balanceOf(address(this));
        IIziCallback(msg.sender).swapY2XCallback(amountX, amountY, data);
        require(ITokenI(tokenY).balanceOf(address(this)) >= by + amountY, "YE");
    }
}
