// SPDX-License-Identifier: MIT
// Адаптер Fraxswap V2 для вызова через swapFrom штатным ArbExecutor V5.
// Долгосрочные ордера исполняются до измерения входа; баланс TWAMM принадлежит ордерам.
pragma solidity 0.8.37;

interface IFraxRegistry {
    function getPair(address tokenIn, address tokenOut) external view returns (address);
}

interface IFraxMarket {
    function factory() external view returns (address);
    function token0() external view returns (address);
    function token1() external view returns (address);
    function executeVirtualOrders(uint256 timestamp) external;
    function getAmountOut(uint256 amount, address tokenIn) external view returns (uint256);
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata data) external;
}

interface IFraxToken {
    function balanceOf(address account) external view returns (uint256);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

contract FraxswapAdapter {
    // Устанавливается конструктором один раз. Фабрика проверяется при подключении конкретного адаптера.
    IFraxRegistry public immutable factory;
    bool transient private entered;

    constructor(IFraxRegistry registry) {
        require(address(registry).code.length > 0, "factory");
        factory = registry;
    }

    function swapFrom(address tokenIn, address tokenOut, uint256 amount, uint256 minimum,
                      address recipient, uint256 deadline) external returns (uint256 received) {
        require(!entered && block.timestamp <= deadline && tokenIn != tokenOut && amount > 0, "order");
        entered = true;
        IFraxMarket pair = IFraxMarket(factory.getPair(tokenIn, tokenOut));
        bool zeroIn = tokenIn < tokenOut;
        require(address(pair) != address(0) && pair.factory() == address(factory), "market");
        require(pair.token0() == (zeroIn ? tokenIn : tokenOut) &&
                pair.token1() == (zeroIn ? tokenOut : tokenIn), "tokens");
        require(recipient != address(0) && recipient != address(pair), "recipient");

        pair.executeVirtualOrders(block.timestamp);
        IFraxToken input = IFraxToken(tokenIn);
        IFraxToken output = IFraxToken(tokenOut);
        uint256 beforePair = input.balanceOf(address(pair));
        uint256 beforePayer = input.balanceOf(msg.sender);
        uint256 beforeRecipient = output.balanceOf(recipient);
        (bool ok, bytes memory data) = tokenIn.call(
            abi.encodeCall(input.transferFrom, (msg.sender, address(pair), amount))
        );
        require(ok && (data.length == 0 || abi.decode(data, (bool))), "transfer");
        require(beforePayer - input.balanceOf(msg.sender) == amount, "spent");
        uint256 actual = input.balanceOf(address(pair)) - beforePair;
        uint256 quoted = pair.getAmountOut(actual, tokenIn);
        require(quoted > 0, "empty");
        pair.swap(zeroIn ? 0 : quoted, zeroIn ? quoted : 0, recipient, "");
        received = output.balanceOf(recipient) - beforeRecipient;
        require(received > 0 && received >= minimum, "payout");
        entered = false;
    }
}
