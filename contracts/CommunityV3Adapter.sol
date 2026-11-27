// SPDX-License-Identifier: MIT
// Исполняет обмен одного стандартного V3-пула через фабрику, указанную при размещении владельцем.
// Обратный вызов оплачивает фактический вход плательщика. Проверка выплаты, срока и лимита реализована в RegisteredV3Adapter.
pragma solidity 0.8.37;
import {RegisteredV3Adapter,IRegisteredV3Factory} from "./RegisteredV3Adapter.sol";

contract CommunityV3Adapter is RegisteredV3Adapter {
    constructor(IRegisteredV3Factory registry) RegisteredV3Adapter(registry) {}

    function uniswapV3SwapCallback(int256 amount0,int256 amount1,bytes calldata data) external {
        pay(amount0,amount1,data);
    }
}
