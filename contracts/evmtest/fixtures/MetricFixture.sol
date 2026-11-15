// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/MetricFixture.sol
// Что делает: ТЕСТОВЫЙ пул вида Metric OMM (фикстура) для EVM-теста шага METRIC ArbExecutor v2 (решение 285):
//   исходник пулов Metric на BSC не опубликован — повторены только подпись swap и порядок оплаты, установленные фактом
//   (ABI SDK @nradko/metric-omm-sdk-v1@0.4.5 + проба на цепи, tests/data/metric_2026-11-15): swap(recipient,
//   zeroForOne, amountSpecified > 0 — точный вход, priceLimitX64, callbackData, extensionData); выход переводится
//   получателю, затем у вызывающего metricOmmSwapCallback(d0, d1, callbackData) с положительной дельтой входа; проверка
//   «баланс входа вырос на долг». Курс 1:1. Порядок токенов задаётся конструктором как есть (у пулов Metric token0 — не
//   обязательно меньший адрес). В сети не разворачивается.
// Кто вызывает: contracts/evmtest/metric.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

interface ITokenM {
    function transfer(address to, uint256 v) external returns (bool);
    function balanceOf(address a) external view returns (uint256);
}

interface IMetricCallback {
    function metricOmmSwapCallback(int256 d0, int256 d1, bytes calldata data) external;
}

contract MetricFixture {
    address public token0;
    address public token1;

    constructor(address a, address b) {
        token0 = a;
        token1 = b;
    }

    function swap(address recipient, bool zeroForOne, int128 amountSpecified, uint128, bytes calldata cb, bytes calldata)
        external
        returns (int128, int128)
    {
        require(amountSpecified > 0, "exact in");
        uint256 amt = uint256(int256(amountSpecified));
        (address tin, address tout) = zeroForOne ? (token0, token1) : (token1, token0);
        ITokenM(tout).transfer(recipient, amt);
        uint256 before = ITokenM(tin).balanceOf(address(this));
        int256 d = int256(amt);
        if (zeroForOne) IMetricCallback(msg.sender).metricOmmSwapCallback(d, -d, cb);
        else IMetricCallback(msg.sender).metricOmmSwapCallback(-d, d, cb);
        require(ITokenM(tin).balanceOf(address(this)) >= before + amt, "IIA");
        return zeroForOne ? (int128(d), -int128(d)) : (-int128(d), int128(d));
    }
}
