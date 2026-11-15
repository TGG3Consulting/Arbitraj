// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/SmardexFixture.sol
// Что делает: ТЕСТОВАЯ пара вида SmarDex (фикстура) для EVM-теста шага SMARDEX ArbExecutor v2 (решение 288): официальный
//   SmardexPair (SmarDex-Ecosystem/dex-smart-contracts@74d0444, solc 0.8.17) в репозиторий не скопирован — повторены
//   только подпись и порядок обмена (SmardexPair.sol L155-L319): swap(to, zeroForOne, amountSpecified > 0, data) — «to» не
//   токен пары (L175), выход переводится «to» ДО обратного вызова, затем smardexSwapCallback(amount0, amount1, data) у
//   вызывающего (положительная дельта — долг пары), затем проверка, что баланс входа вырос на долг (L268-L274, L280-L286).
//   Курс — 99 % от входа (кривую фиктивных резервов проверяют тесты бота на записанных ответах цепи). В сети не разворачивается.
// Кто вызывает: contracts/evmtest/smardex.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

interface ITokenS {
    function transfer(address to, uint256 v) external returns (bool);
    function balanceOf(address a) external view returns (uint256);
}

interface ISmardexSwapCallbackS {
    function smardexSwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata data) external;
}

contract SmardexFixture {
    address public token0;
    address public token1;

    constructor(address a, address b) {
        (token0, token1) = a < b ? (a, b) : (b, a);
    }

    function swap(address to, bool zeroForOne, int256 amountSpecified, bytes calldata data)
        external
        returns (int256 amount0, int256 amount1)
    {
        require(amountSpecified > 0, "fixture: exact in only");
        require(to != token0 && to != token1, "SmarDex: INVALID_TO");
        (address tin, address tout) = zeroForOne ? (token0, token1) : (token1, token0);
        uint256 a = uint256(amountSpecified);
        uint256 out = a * 99 / 100;
        uint256 before = ITokenS(tin).balanceOf(address(this));
        (amount0, amount1) = zeroForOne ? (amountSpecified, -int256(out)) : (-int256(out), amountSpecified);
        ITokenS(tout).transfer(to, out);
        ISmardexSwapCallbackS(msg.sender).smardexSwapCallback(amount0, amount1, data);
        require(ITokenS(tin).balanceOf(address(this)) >= before + a, "SmarDex: INSUFFICIENT_INPUT_AMOUNT");
    }
}
