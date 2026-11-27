// SPDX-License-Identifier: MIT
// Владелец развёртывает модуль для конкретной пары Temple и её проверенного роутера.
// Защищает валюту и чистую сумму выплаты при переключении между AMM и казначейством.
pragma solidity 0.8.37;

interface ITempleMarket {
    function token0() external view returns (address);
    function token1() external view returns (address);
    function router() external view returns (address);
}

interface ITempleVenue {
    function tokenPair(address stable) external view returns (address);
    function templeToken() external view returns (address);
    function defendStable() external view returns (address);
    function swapExactTempleForStableQuote(address market, uint256 amount) external view returns (bool, uint256);
    function swapExactStableForTemple(uint256 amount, uint256 minimum, address stable, address to, uint256 deadline) external returns (uint256);
    function swapExactTempleForStable(uint256 amount, uint256 minimum, address stable, address to, uint256 deadline) external returns (uint256);
}

interface ITempleAsset {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function approve(address spender, uint256 amount) external returns (bool);
    function balanceOf(address holder) external view returns (uint256);
    function allowance(address holder, address spender) external view returns (uint256);
}

contract TempleAdapter {
    ITempleMarket public immutable pair;
    ITempleVenue public immutable router;
    bool transient private entered;

    constructor(ITempleMarket market, ITempleVenue venue) {
        require(address(market).code.length != 0 && address(venue).code.length != 0, "code");
        require(venue.tokenPair(market.token1()) == address(market), "market");
        require(venue.templeToken() == market.token0() && market.token0() != market.token1(), "tokens");
        pair = market;
        router = venue;
    }

    function tokenCall(address token, bytes memory data) private {
        (bool ok, bytes memory value) = token.call(data);
        require(ok && token.code.length > 0 && (value.length == 0 || abi.decode(value,(bool))), "token");
    }

    function swapFrom(address tokenIn, address tokenOut, uint256 amount, uint256 minimum,
                      address recipient, uint256 deadline) external returns (uint256 received) {
        require(!entered && block.timestamp <= deadline && amount > 0, "order");
        require(recipient != address(0) && recipient != address(this) && tokenIn != tokenOut, "recipient");
        entered = true;
        address stable = pair.token1();
        address temple = pair.token0();
        require(router.tokenPair(stable) == address(pair) && router.templeToken() == temple, "market");
        bool buy = tokenIn == stable && tokenOut == temple;
        bool defend;
        if (!buy) {
            require(tokenIn == temple, "input");
            uint256 expected;
            (defend, expected) = router.swapExactTempleForStableQuote(address(pair), amount);
            require(tokenOut == (defend ? router.defendStable() : stable) && expected >= minimum, "settlement");
        }
        if (!defend) require(pair.router() == address(router), "pair router");
        ITempleAsset input = ITempleAsset(tokenIn);
        uint256 initial = input.balanceOf(address(this));
        uint256 payer = input.balanceOf(msg.sender);
        uint256 beforeOut = ITempleAsset(tokenOut).balanceOf(recipient);
        tokenCall(tokenIn, abi.encodeCall(input.transferFrom, (msg.sender, address(this), amount)));
        require(input.balanceOf(address(this)) - initial == amount, "funding");
        tokenCall(tokenIn, abi.encodeCall(input.approve, (address(router), amount)));
        // Покупка через оригинальный роутер возвращает ноль в ABI даже при реальной выплате.
        if (buy) router.swapExactStableForTemple(amount, minimum, stable, recipient, deadline);
        else router.swapExactTempleForStable(amount, minimum, stable, recipient, deadline);
        if (input.allowance(address(this), address(router)) != 0) {
            tokenCall(tokenIn, abi.encodeCall(input.approve, (address(router), 0)));
            require(input.allowance(address(this), address(router)) == 0, "allowance");
        }
        require(input.balanceOf(address(this)) == initial && payer - input.balanceOf(msg.sender) == amount, "debit");
        received = ITempleAsset(tokenOut).balanceOf(recipient) - beforeOut;
        require(received != 0 && received >= minimum, "credit");
        entered = false;
    }
}
