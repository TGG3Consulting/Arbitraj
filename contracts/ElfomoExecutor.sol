// SPDX-License-Identifier: MIT
// Файл: contracts/ElfomoExecutor.sol.
// Что делает: исполняет полный обмен ElfomoFi Base с минимумом по фактическому приросту баланса кошелька.
// Кто вызывает: кошелёк через штатный прямой путь исполнения. От чего зависит: фиксированные router, vault и три ERC-20.
// Настройки: dex.kind, dex.router и dex.deadline_s. Адрес модуля служит taker для расчёта цены.
pragma solidity 0.8.37;

interface IElfomoRouter {
    function getAmountOut(address from, address to, uint256 amount) external view returns (uint256);
    function swap(address from, address to, int256 amount, uint256 minimum, address receiver, uint256 partner) external;
}

interface IElfomoToken {
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

contract ElfomoExecutor {
    address private constant ROUTER = address(bytes20(hex"f0f0f0f0fb0d738452efd03a28e8be14c76d5f73"));
    address private constant VAULT = address(bytes20(hex"bb1b19f138db3925883a96ff7a304277460e0c99"));
    address private constant USDC = address(bytes20(hex"833589fcd6edb6e08f4c7c32d4f71b54bda02913"));
    address private constant WETH = address(bytes20(hex"4200000000000000000000000000000000000006"));
    address private constant CBBTC = address(bytes20(hex"cbb7c0000ab88b473b1f5afd9ef808440eed33bf"));
    bool private trading;

    error InvalidOrder();
    error UnsupportedToken();
    error InsufficientOutput();
    error ExhaustedVault();
    error PartialFill();

    function supported(address tokenIn, address tokenOut) private pure returns (bool) {
        return (tokenIn == USDC && (tokenOut == WETH || tokenOut == CBBTC)) ||
               (tokenOut == USDC && (tokenIn == WETH || tokenIn == CBBTC));
    }

    // Котировка и обмен видны роутеру от одного адреса. Внешний eth_call передаёт фактический кошелёк.
    function getAmountOut(address tokenIn, address tokenOut, uint256 amount) public view returns (uint256 quoted) {
        if (block.chainid != 8453 || !supported(tokenIn, tokenOut)) revert UnsupportedToken();
        quoted = IElfomoRouter(ROUTER).getAmountOut(tokenIn, tokenOut, amount);
        IElfomoToken output = IElfomoToken(tokenOut);
        if (quoted >= output.balanceOf(VAULT) || quoted >= output.allowance(VAULT, ROUTER)) revert ExhaustedVault();
    }

    function execute(address market, address tokenIn, address tokenOut, uint256 amountIn,
                     uint256 minOut, uint256 deadline) external returns (uint256 received) {
        if (block.chainid != 8453 || market != ROUTER || trading || deadline < block.timestamp ||
            minOut == 0 || amountIn == 0 || amountIn > uint256(type(int256).max)) revert InvalidOrder();
        if (!supported(tokenIn, tokenOut)) revert UnsupportedToken();
        trading = true;
        IElfomoToken input = IElfomoToken(tokenIn);
        IElfomoToken output = IElfomoToken(tokenOut);
        uint256 initialIn = input.balanceOf(msg.sender);
        uint256 initialOut = output.balanceOf(msg.sender);
        uint256 original = input.balanceOf(address(this));
        if (input.allowance(address(this), ROUTER) != 0) revert UnsupportedToken();
        checkedCall(tokenIn, abi.encodeCall(input.transferFrom, (msg.sender, address(this), amountIn)));
        if (input.balanceOf(address(this)) != original + amountIn) revert PartialFill();
        checkedCall(tokenIn, abi.encodeCall(input.approve, (ROUTER, amountIn)));
        if (getAmountOut(tokenIn, tokenOut, amountIn) < minOut) revert InsufficientOutput();
        IElfomoRouter(ROUTER).swap(tokenIn, tokenOut, int256(amountIn), minOut, msg.sender, 0);
        checkedCall(tokenIn, abi.encodeCall(input.approve, (ROUTER, 0)));
        if (input.allowance(address(this), ROUTER) != 0) revert UnsupportedToken();
        if (input.balanceOf(address(this)) != original || initialIn - input.balanceOf(msg.sender) != amountIn)
            revert PartialFill();
        received = output.balanceOf(msg.sender) - initialOut;
        if (received < minOut) revert InsufficientOutput();
        trading = false;
    }

    function checkedCall(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        if (!ok || (result.length != 0 && !abi.decode(result, (bool)))) revert UnsupportedToken();
    }
}
