// SPDX-License-Identifier: MIT
// Принимает штатный обратный вызов Yum Yum Swap и оплачивает только активный обмен.
// Кошелёк или ArbExecutor V5 вызывает модуль, пул вызывает его при оплате.
// Модуль для фабрики развёртывает владелец.
pragma solidity 0.8.37;
import {RegisteredV3Adapter,IRegisteredV3Factory} from "./RegisteredV3Adapter.sol";

contract YumyumAdapter is RegisteredV3Adapter {
    constructor(IRegisteredV3Factory registry) RegisteredV3Adapter(registry) {}

    function yumyumswapSwapCallback(int256 amount0,int256 amount1,bytes calldata data) external {
        pay(amount0,amount1,data);
    }
}
