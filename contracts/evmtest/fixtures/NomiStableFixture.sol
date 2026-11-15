// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/NomiStableFixture.sol
// Что делает: ТЕСТОВАЯ пара вида Nomiswap Stable (фикстура) для EVM-теста шага NOMI_STABLE ArbExecutor v2 (решение
//   284): официальный NomiswapStablePair (Sourcify 56/0x1e40450f8e21bb68490d7d91ab422888fb3d60f1, solc 0.8.15) в
//   репозиторий не скопирован — повторены только строки обмена и интерфейс: getReserves (uint112), getAmountOut(address
//   tokenIn, uint256 amountIn) (L251) — комиссия со входа `(in·MAX_FEE − in·swapFee) / MAX_FEE` (L257), кривая 1:1
//   вместо стейблсвапа (кривая не проверяется — её проверяют тесты бота на записанных ответах пары); swap(out0, out1,
//   to, data) (L132-L176): выход переводится до проверки, вход = баланс − (резерв − выход) (L149-L151), «есть вход»
//   (L151), проверка инварианта по балансу со снятой комиссией (здесь 1:1: вход·(1 − fee) ≥ выход), _update — резервы
//   = балансы (L313-L319). Метода Solidly getAmountOut(uint256,address) нет (как у настоящей пары — PUSH4 0xf140a35a в
//   коде пары нет, tests/data/nomiswap_stable_2026-11-15/exec_sim.json). В сети не разворачивается.
// Кто вызывает: contracts/evmtest/nomi.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

interface ITokenN {
    function transfer(address to, uint256 v) external returns (bool);
    function balanceOf(address a) external view returns (uint256);
}

contract NomiStableFixture {
    uint256 internal constant MAX_FEE = 100000;
    address public token0;
    address public token1;
    uint112 internal reserve0;
    uint112 internal reserve1;
    uint32 public swapFee;

    constructor(address a, address b, uint32 fee) {
        (token0, token1) = a < b ? (a, b) : (b, a);
        swapFee = fee;
    }

    function getReserves() external view returns (uint112, uint112, uint32) {
        return (reserve0, reserve1, 0);
    }

    function sync() external {
        reserve0 = uint112(ITokenN(token0).balanceOf(address(this)));
        reserve1 = uint112(ITokenN(token1).balanceOf(address(this)));
    }

    function getAmountOut(address tokenIn, uint256 amountIn) external view returns (uint256) {
        require(tokenIn == token0 || tokenIn == token1, "INVALID_INPUT_TOKEN");
        return (amountIn * MAX_FEE - amountIn * swapFee) / MAX_FEE;
    }

    function swap(uint256 out0, uint256 out1, address to, bytes calldata) external {
        require(out0 > 0 || out1 > 0, "INSUFFICIENT_OUTPUT_AMOUNT");
        (uint256 r0, uint256 r1) = (reserve0, reserve1);
        require(out0 < r0 && out1 < r1, "INSUFFICIENT_LIQUIDITY");
        if (out0 > 0) ITokenN(token0).transfer(to, out0);
        if (out1 > 0) ITokenN(token1).transfer(to, out1);
        uint256 b0 = ITokenN(token0).balanceOf(address(this));
        uint256 b1 = ITokenN(token1).balanceOf(address(this));
        uint256 in0 = b0 > r0 - out0 ? b0 - (r0 - out0) : 0;
        uint256 in1 = b1 > r1 - out1 ? b1 - (r1 - out1) : 0;
        require(in0 > 0 || in1 > 0, "INSUFFICIENT_INPUT_AMOUNT");
        // Инвариант 1:1 со снятой комиссией входа: (вход·(MAX_FEE − fee)) / MAX_FEE ≥ выход по каждой стороне.
        require((in0 * (MAX_FEE - swapFee)) / MAX_FEE >= out1 && (in1 * (MAX_FEE - swapFee)) / MAX_FEE >= out0, "D");
        reserve0 = uint112(b0);
        reserve1 = uint112(b1);
    }
}
