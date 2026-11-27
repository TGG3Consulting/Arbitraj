// SPDX-License-Identifier: MIT
// Штатный spot swap Ammalgam. Модуль ограничивает срок и измеряет остатки кошелька.
// Шаг 29 существующего V5 имеет собственный итоговый минимум и выполняется атомарно.
pragma solidity 0.8.37;

interface IAmmalgamRegistry {
    function getPair(address input, address output) external view returns (address);
    function pairBeacon() external view returns (address);
}
interface IAmmalgamBeacon {
    function implementation() external view returns (address);
}
interface IAmmalgamMarket {
    function underlyingTokens() external view returns (address, address);
}
interface IAmmalgamHelper {
    struct Swap {address to; address tokenIn; address tokenOut; uint256 amountIn; uint256 minAmountOut;}
    function swapHelper(Swap calldata request) external;
}
interface IAmmalgamToken {
    function allowance(address owner,address spender) external view returns (uint256);
    function balanceOf(address who) external view returns (uint256);
    function approve(address spender,uint256 amount) external returns (bool);
    function transferFrom(address payer,address to,uint256 amount) external returns (bool);
}

contract AmmalgamWallet {
    IAmmalgamRegistry public immutable factory;
    IAmmalgamHelper public immutable helper;
    address public immutable beacon;
    bytes32 public immutable helperHash;
    bytes32 public immutable executorHash;
    bytes32 public immutable pairHash;
    bytes32 public immutable implementationHash;
    bool transient private entered;
    error InvalidMarket();
    error InvalidOrder();
    error Payment();
    event Filled(address indexed pair,address indexed payer,address indexed recipient,address input,address output,
                 uint256 spent,uint256 funded,uint256 received);

    constructor(IAmmalgamRegistry registry, IAmmalgamHelper venue, IAmmalgamMarket referencePair, bytes32 executorRuntime) {
        if (executorRuntime == bytes32(0)) revert InvalidMarket();
        executorHash = executorRuntime;
        if (address(registry).code.length == 0 || address(venue).code.length == 0) revert InvalidMarket();
        (address x,address y) = referencePair.underlyingTokens();
        if (x == y || registry.getPair(x,y) != address(referencePair)) revert InvalidMarket();
        factory = registry;
        helper = venue;
        beacon = registry.pairBeacon();
        address impl = IAmmalgamBeacon(beacon).implementation();
        if (impl.code.length == 0) revert InvalidMarket();
        implementationHash = impl.codehash;
        pairHash = address(referencePair).codehash;
        helperHash = address(venue).codehash;
    }

    function execute(address pair,address input,address output,uint256 amount,uint256 minimum,uint256 deadline)
        external returns (uint256) {
        if (minimum == 0) revert InvalidOrder();
        return settle(pair,input,output,amount,minimum,msg.sender,deadline);
    }

    function swapFrom(address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        external returns (uint256) {
        if (minimum == 0 && msg.sender.codehash != executorHash) revert InvalidOrder();
        return settle(factory.getPair(input,output),input,output,amount,minimum,recipient,deadline);
    }

    function checked(address token, bytes memory data) private {
        (bool ok,bytes memory value) = token.call(data);
        if (!ok || (value.length != 0 && !abi.decode(value,(bool)))) revert Payment();
    }

    function settle(address pair,address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        private returns (uint256 received) {
        if (entered || block.timestamp > deadline || amount == 0 || input == output || recipient == address(0)
            || recipient == pair || recipient == address(this) || recipient == address(helper)) revert InvalidOrder();
        if (pair == address(0) || pair.codehash != pairHash || factory.getPair(input,output) != pair
            || factory.pairBeacon() != beacon || IAmmalgamBeacon(beacon).implementation().codehash != implementationHash
            || address(helper).codehash != helperHash) revert InvalidMarket();
        (address x,address y) = IAmmalgamMarket(pair).underlyingTokens();
        if (!((x == input && y == output) || (x == output && y == input))) revert InvalidMarket();
        entered = true;
        IAmmalgamToken coin = IAmmalgamToken(input);
        uint256 beforeIn = coin.balanceOf(msg.sender);
        uint256 inventory = coin.balanceOf(address(this));
        uint256 beforeOut = IAmmalgamToken(output).balanceOf(recipient);
        if (coin.allowance(address(this),address(helper)) != 0) revert Payment();
        checked(input,abi.encodeCall(coin.transferFrom,(msg.sender,address(this),amount)));
        uint256 funded = coin.balanceOf(address(this)) - inventory;
        if (funded == 0 || funded > amount) revert Payment();
        checked(input,abi.encodeCall(coin.approve,(address(helper),funded)));
        helper.swapHelper(IAmmalgamHelper.Swap(recipient,input,output,funded,minimum));
        if (coin.allowance(address(this),address(helper)) != 0) {
            checked(input,abi.encodeCall(coin.approve,(address(helper),0)));
            if (coin.allowance(address(this),address(helper)) != 0) revert Payment();
        }
        received = IAmmalgamToken(output).balanceOf(recipient) - beforeOut;
        if (beforeIn - coin.balanceOf(msg.sender) != amount || coin.balanceOf(address(this)) != inventory
            || received == 0 || received < minimum) revert Payment();
        emit Filled(pair,msg.sender,recipient,input,output,amount,funded,received);
        entered = false;
    }
}
