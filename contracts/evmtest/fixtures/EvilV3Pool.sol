// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/EvilV3Pool.sol
// Что делает: ТЕСТОВЫЙ пул вида V3 (фикстура) для EVM-теста ArbExecutor (ревью 272, А4): официального кода пула V3 в
//   репозитории нет, повторена только форма обмена Uniswap V3 (v3-core@5ea68a77 UniswapV3Pool.sol L596-L788: точный
//   вход amountSpecified > 0, выход переводится получателю ДО обратного вызова, затем uniswapV3SwapCallback(d0, d1) с
//   положительной дельтой входа и проверка «баланс входа вырос на долг» — require IIA). Курс 1:1 (цена не проверяется).
//   Режимы: 0 — честный; 1 — обратный вызов дважды; 2 — долг на 1 больше суммы шага; 3 — взял вход и не отдал выход.
//   В сети не разворачивается.
// Кто вызывает: contracts/evmtest/review.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

interface IToken3 {
    function transfer(address to, uint256 v) external returns (bool);
    function balanceOf(address a) external view returns (uint256);
}

interface ISwapCallback3 {
    function uniswapV3SwapCallback(int256 d0, int256 d1, bytes calldata data) external;
}

contract EvilV3Pool {
    address public token0;
    address public token1;
    uint8 public mode;

    constructor(address a, address b, uint8 m) {
        (token0, token1) = a < b ? (a, b) : (b, a);
        mode = m;
    }

    function swap(address to, bool zeroForOne, int256 amountSpecified, uint160, bytes calldata)
        external
        returns (int256 d0, int256 d1)
    {
        require(amountSpecified > 0, "exact in");
        uint256 amt = uint256(amountSpecified);
        (address tin, address tout) = zeroForOne ? (token0, token1) : (token1, token0);
        if (mode != 3) require(IToken3(tout).transfer(to, amt), "out");
        int256 owed = int256(mode == 2 ? amt + 1 : amt);
        (d0, d1) = zeroForOne ? (owed, -int256(amt)) : (-int256(amt), owed);
        uint256 had = IToken3(tin).balanceOf(address(this));
        ISwapCallback3(msg.sender).uniswapV3SwapCallback(d0, d1, "");
        if (mode == 1) ISwapCallback3(msg.sender).uniswapV3SwapCallback(d0, d1, "");
        require(IToken3(tin).balanceOf(address(this)) >= had + uint256(owed), "IIA");
    }
}
