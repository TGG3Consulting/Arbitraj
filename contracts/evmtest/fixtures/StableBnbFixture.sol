// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/StableBnbFixture.sol
// Что делает: ТЕСТОВЫЙ пул (фикстура) только для EVM-теста ArbExecutor, шаг STABLE_U256_BNB внутри executeNative
//   (решение 272). Кода пула PancakeSwap Stable / Lista в репозитории нет (репозиторий Pancake закрыт), поэтому
//   повторены ровно те строки официальных исходников, которые касаются BNB: монета k = BNB_ADDRESS 0xEeee…; вход BNB —
//   require(dx == msg.value) (PancakeStableSwapTwoPool.sol L457-L460, Lista StableSwapPool.sol L507-L510); вход ERC-20 —
//   transferFrom(msg.sender, пул, dx); выход BNB — msg.sender.call{gas: bnb_gas, value: dy}("") (TwoPool L644-L663,
//   Lista L697-L716); выход ERC-20 — transfer(msg.sender, dy). Курс 1:1 (математика StableSwap здесь не проверяется —
//   она проверена в тестах бота на записанных пулах). В сети не разворачивается.
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: ничего.
pragma solidity 0.8.37;

interface IToken {
    function transfer(address to, uint256 v) external returns (bool);
    function transferFrom(address f, address t, uint256 v) external returns (bool);
}

contract StableBnbFixture {
    address internal constant BNB_ADDRESS = 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;
    address[2] public coins;
    uint256 public bnb_gas;

    constructor(address token, uint256 gasForBnb) payable {
        coins[0] = BNB_ADDRESS;
        coins[1] = token;
        bnb_gas = gasForBnb;
    }

    function exchange(uint256 i, uint256 j, uint256 dx, uint256 minDy) external payable {
        if (coins[i] == BNB_ADDRESS) {
            require(dx == msg.value, "Inconsistent quantity");
        } else {
            require(msg.value == 0, "Inconsistent quantity");
            require(IToken(coins[i]).transferFrom(msg.sender, address(this), dx), "in");
        }
        uint256 dy = dx;
        require(dy >= minDy, "Exchange resulted in fewer coins than expected");
        if (coins[j] == BNB_ADDRESS) {
            (bool ok,) = msg.sender.call{gas: bnb_gas, value: dy}("");
            require(ok, "BNB transfer failed");
        } else {
            require(IToken(coins[j]).transfer(msg.sender, dy), "out");
        }
    }

    receive() external payable {}
}
