// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;
import {RegisteredV3Adapter,IRegisteredV3Factory} from "./RegisteredV3Adapter.sol";

contract BluedexAdapter is RegisteredV3Adapter {
    constructor(IRegisteredV3Factory registry) RegisteredV3Adapter(registry) {}

    function bluedexV3SwapCallback(int256 amount0,int256 amount1,bytes calldata data) external {
        pay(amount0,amount1,data);
    }
}
