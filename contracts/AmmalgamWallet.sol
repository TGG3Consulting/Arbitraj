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
    function getReserves() external view returns (uint112, uint112, uint32);
    function referenceReserves() external view returns (uint112, uint112);
    function totalAssetsAndShares(bool withInterest) external view returns (uint112[6] memory,uint112[6] memory);
    function swap(uint256 outX,uint256 outY,address recipient,bytes calldata data) external;
}
interface IAmmalgamHelper {
    function computeExpectedSwapAmountOut(uint256 input,uint256 reserveIn,uint256 referenceIn,
        uint256 reserveOut,uint256 missingIn,uint256 missingOut) external pure returns(uint256);
}
interface IAmmalgamToken {
    function balanceOf(address who) external view returns (uint256);
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

    // TokenController.getReserves and Pair.swap use INTEREST_PERIOD_FOR_SWAP.
    // https://repo.sourcify.dev/1/0x5C0d819609Eee2cdCf326f0e898A8447e06a45Be
    uint32 private constant SWAP_INTEREST_PERIOD = 1 days - 1;
    struct Curve {uint256 reserveIn; uint256 refIn; uint256 reserveOut; uint256 missingIn; uint256 missingOut;}

    function curve(IAmmalgamMarket pair,bool inputX) private view returns(Curve memory) {
        (uint112 x,uint112 y,uint32 last) = pair.getReserves();
        (uint112 refX,uint112 refY) = pair.referenceReserves();
        uint32 elapsed;
        unchecked {elapsed = uint32(block.timestamp) - last;}
        (uint112[6] memory assets,) = pair.totalAssetsAndShares(elapsed >= SWAP_INTEREST_PERIOD);
        uint256 mx = assets[4] > assets[1] ? assets[4] - assets[1] : 0;
        uint256 my = assets[5] > assets[2] ? assets[5] - assets[2] : 0;
        return inputX ? Curve(x,refX,y,mx,my) : Curve(y,refY,x,my,mx);
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
        uint256 poolBefore = coin.balanceOf(pair);
        uint256 beforeOut = IAmmalgamToken(output).balanceOf(recipient);
        Curve memory c = curve(IAmmalgamMarket(pair), x == input);
        checked(input,abi.encodeCall(coin.transferFrom,(msg.sender,pair,amount)));
        uint256 funded = coin.balanceOf(pair) - poolBefore;
        if (funded == 0 || funded > amount) revert Payment();
        uint256 credit = helper.computeExpectedSwapAmountOut(funded,c.reserveIn,c.refIn,c.reserveOut,c.missingIn,c.missingOut);
        uint256 available = IAmmalgamToken(output).balanceOf(pair);
        if (c.reserveOut <= c.missingOut) revert Payment();
        uint256 strictLimit = c.reserveOut - c.missingOut - 1;
        if (credit > strictLimit) credit = strictLimit;
        if (credit > available) credit = available;
        if (credit == 0 || credit < minimum) revert Payment();
        IAmmalgamMarket(pair).swap(x == input ? 0 : credit, x == input ? credit : 0,recipient,"");
        received = IAmmalgamToken(output).balanceOf(recipient) - beforeOut;
        if (beforeIn - coin.balanceOf(msg.sender) != amount
            || received == 0 || received < minimum) revert Payment();
        emit Filled(pair,msg.sender,recipient,input,output,amount,funded,received);
        entered = false;
    }
}
