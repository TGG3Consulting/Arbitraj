// SPDX-License-Identifier: MIT
// Файл: contracts/SolidlyV3Executor.sol.
// Что делает: исполняет одну полную покупку или продажу Solidly V3, сверяет вход и выход по балансам плательщика.
// Кто вызывает: кошелёк через exec/bundle.rs; exec/fork_swap.rs строит пробу того же вызова.
// От чего зависит: каноническая фабрика Solidly V3 на Ethereum и Base, шестипараметрический swap пула и ERC-20.
// Настройки: dex.kind=solidly_v3_module, dex.router, dex.pool_enabled; хеш runtime сверяет pools/solidly_v3.rs.
pragma solidity 0.8.37;

interface ISolidlyV3Factory {
    function getPool(address token0, address token1, int24 tickSpacing) external view returns (address);
}

interface ISolidlyV3Pool {
    function factory() external view returns (address);
    function token0() external view returns (address);
    function token1() external view returns (address);
    function tickSpacing() external view returns (int24);
    function swap(address recipient, bool zeroForOne, int256 amountSpecified, uint160 sqrtPriceLimitX96,
                  uint256 amountLimit, uint256 deadline) external returns (int256 amount0, int256 amount1);
}

interface ISolidlyV3Token {
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transferFrom(address owner, address recipient, uint256 amount) external returns (bool);
}

contract SolidlyV3Executor {
    ISolidlyV3Factory public constant FACTORY = ISolidlyV3Factory(0x70Fe4a44EA505cFa3A57b95cF2862D4fd5F0f687);
    uint160 private constant MIN_PRICE = 4295128740;
    uint160 private constant MAX_PRICE = 1461446703485210103287273052203988822378723970341;
    bool private trading;

    error InvalidOrder();
    error UnregisteredPool();
    error UnsupportedToken();
    error PartialFill();
    error InsufficientOutput();

    function execute(address pool, address tokenIn, address tokenOut, uint256 amountIn, uint256 minOut,
                     uint256 deadline) external returns (uint256 received) {
        if ((block.chainid != 8453 && block.chainid != 1) || trading || deadline < block.timestamp ||
            tokenIn == tokenOut || minOut == 0 || amountIn == 0 || amountIn > uint256(type(int256).max))
            revert InvalidOrder();
        trading = true;
        ISolidlyV3Pool market = ISolidlyV3Pool(pool);
        address first = market.token0();
        address second = market.token1();
        bool zeroForOne = tokenIn == first && tokenOut == second;
        if ((!zeroForOne && !(tokenIn == second && tokenOut == first)) || market.factory() != address(FACTORY) ||
            FACTORY.getPool(first, second, market.tickSpacing()) != pool) revert UnregisteredPool();

        ISolidlyV3Token input = ISolidlyV3Token(tokenIn);
        ISolidlyV3Token output = ISolidlyV3Token(tokenOut);
        uint256 original = input.balanceOf(address(this));
        uint256 payerIn = input.balanceOf(msg.sender);
        uint256 payerOut = output.balanceOf(msg.sender);
        if (input.allowance(address(this), pool) != 0) revert UnsupportedToken();
        checkedCall(tokenIn, abi.encodeCall(input.transferFrom, (msg.sender, address(this), amountIn)));
        if (input.balanceOf(address(this)) != original + amountIn) revert UnsupportedToken();
        checkedCall(tokenIn, abi.encodeCall(input.approve, (pool, amountIn)));
        (int256 amount0, int256 amount1) = market.swap(msg.sender, zeroForOne, int256(amountIn),
            zeroForOne ? MIN_PRICE : MAX_PRICE, minOut, deadline);
        checkedCall(tokenIn, abi.encodeCall(input.approve, (pool, 0)));
        if ((zeroForOne ? amount0 : amount1) != int256(amountIn) || input.balanceOf(address(this)) != original ||
            payerIn - input.balanceOf(msg.sender) != amountIn) revert PartialFill();
        if (input.allowance(address(this), pool) != 0) revert UnsupportedToken();
        received = output.balanceOf(msg.sender) - payerOut;
        int256 outputDelta = zeroForOne ? amount1 : amount0;
        if (received < minOut || outputDelta >= 0 || uint256(-outputDelta) != received) revert InsufficientOutput();
        trading = false;
    }

    function checkedCall(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        if (!ok || (result.length != 0 && !abi.decode(result, (bool)))) revert UnsupportedToken();
    }
}
