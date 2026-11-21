// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/DmmFixture.sol
// Что делает: ТЕСТОВЫЙ пул вида KyberSwap Classic DMM (фикстура) для EVM-теста шага KYBER_DMM ArbExecutor v2 (решение
//   287): официальный DMMPool (KyberNetwork/dmm-smart-contracts@e557b57, solc 0.6.12) в репозиторий не скопирован —
//   повторены только строки обмена и интерфейс: getTradeInfo (L240-L265: реальные и виртуальные резервы, комиссия над
//   1e18; здесь комиссия задана постоянной — EMA объёма не моделируется, её проверяют тесты бота на записанных ответах
//   цепи), swap(out0, out1, to, data) (L162-L210): выход строго меньше реального резерва, «to» не токен пула, выход
//   переводится до проверки, вход = баланс − (резерв − выход), виртуальные резервы сдвигаются на изменение реальных,
//   проверка K по виртуальным резервам со снятой со входа комиссией (verifyBalanceAndUpdateEma L285-L309), _update —
//   резервы = балансы. В сети не разворачивается.
// Кто вызывает: contracts/evmtest/dmm.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

interface ITokenD {
    function transfer(address to, uint256 v) external returns (bool);
    function balanceOf(address a) external view returns (uint256);
}

contract DmmFixture {
    uint256 internal constant PRECISION = 1e18;
    address public token0;
    address public token1;
    uint112 internal reserve0;
    uint112 internal reserve1;
    uint112 internal vReserve0;
    uint112 internal vReserve1;
    uint256 internal fee;
    uint256 internal amp; // виртуальный резерв = реальный · amp (при sync)

    constructor(address a, address b, uint256 fee_, uint256 amp_) {
        (token0, token1) = a < b ? (a, b) : (b, a);
        fee = fee_;
        amp = amp_;
    }

    function getTradeInfo() external view returns (uint112, uint112, uint112, uint112, uint256) {
        return (reserve0, reserve1, vReserve0, vReserve1, fee);
    }

    function sync() external {
        reserve0 = uint112(ITokenD(token0).balanceOf(address(this)));
        reserve1 = uint112(ITokenD(token1).balanceOf(address(this)));
        vReserve0 = uint112(reserve0 * amp);
        vReserve1 = uint112(reserve1 * amp);
    }

    function swap(uint256 out0, uint256 out1, address to, bytes calldata) external {
        require(out0 > 0 || out1 > 0, "DMM: INSUFFICIENT_OUTPUT_AMOUNT");
        (uint256 r0, uint256 r1, uint256 v0, uint256 v1) = (reserve0, reserve1, vReserve0, vReserve1);
        require(out0 < r0 && out1 < r1, "DMM: INSUFFICIENT_LIQUIDITY");
        require(to != token0 && to != token1, "DMM: INVALID_TO");
        if (out0 > 0) ITokenD(token0).transfer(to, out0);
        if (out1 > 0) ITokenD(token1).transfer(to, out1);
        uint256 b0 = ITokenD(token0).balanceOf(address(this));
        uint256 b1 = ITokenD(token1).balanceOf(address(this));
        uint256 nv0 = v0 + b0 - r0;
        uint256 nv1 = v1 + b1 - r1;
        uint256 in0 = b0 > r0 - out0 ? b0 - (r0 - out0) : 0;
        uint256 in1 = b1 > r1 - out1 ? b1 - (r1 - out1) : 0;
        require(in0 > 0 || in1 > 0, "DMM: INSUFFICIENT_INPUT_AMOUNT");
        uint256 a0 = (nv0 * PRECISION - in0 * fee) / PRECISION;
        uint256 a1 = (nv1 * PRECISION - in1 * fee) / PRECISION;
        require(a0 * a1 >= v0 * v1, "DMM: K");
        (reserve0, reserve1, vReserve0, vReserve1) = (uint112(b0), uint112(b1), uint112(nv0), uint112(nv1));
    }
}
