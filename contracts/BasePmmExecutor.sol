// SPDX-License-Identifier: MIT
// Файл: contracts/BasePmmExecutor.sol.
// Что делает: исполняет один обмен ElfomoFi, LunarBase или BaiBai, проверяя полный вход и зачисление на кошелёк.
// Кто вызывает: кошелёк, проверки исполнения в GitHub Actions. От чего зависит: канонические адреса Base и ABI рынков.
// Настройки: dex.router, dex.kind и dex.deadline_s. Отдельная архивная приёмка каждого рынка и runtime.
pragma solidity 0.8.37;

import {IElfomoRouter, IElfomoToken, ILunarPool, IBaseWrappedEther, IBaiEntry} from "./IBasePmm.sol";

import {BaseVirtuals} from "./BaseVirtuals.sol";
import {BaseNativeV3} from "./BaseNativeV3.sol";

contract BasePmmExecutor {
    address private constant ELFOMO = address(bytes20(hex"f0f0f0f0fb0d738452efd03a28e8be14c76d5f73"));
    address private constant VAULT = address(bytes20(hex"bb1b19f138db3925883a96ff7a304277460e0c99"));
    address private constant LUNAR = address(bytes20(hex"0000efc4ec03a7c47d3a38a9be7ff1d52dd01b99"));
    address private constant BAI = address(bytes20(hex"98c1d9e102eb2806d902b13186bdc7892ac4ffba"));
    address private constant BAI_VAULT = address(bytes20(hex"aac48feb93c5c97e0fb3c7c57e1633922a4acda3"));
    address private constant NVDAC = address(bytes20(hex"b20000000000000000000078ee7ce2fe4908108c"));
    address private constant USDC = address(bytes20(hex"833589fcd6edb6e08f4c7c32d4f71b54bda02913"));
    address private constant WETH = address(bytes20(hex"4200000000000000000000000000000000000006"));
    address private constant CBBTC = address(bytes20(hex"cbb7c0000ab88b473b1f5afd9ef808440eed33bf"));
    bool private trading;

    error InvalidOrder();
    error UnsupportedToken();
    error InsufficientOutput();
    error ExhaustedVault();
    error PartialFill();
    error ClosedMarket();

    receive() external payable {
        if (!trading || (msg.sender != WETH && msg.sender != LUNAR)) revert InvalidOrder();
    }

    function supported(address market, address a, address b) private pure returns (bool) {
        address other = a == USDC ? b : (b == USDC ? a : address(0));
        return other == WETH || (market == ELFOMO && other == CBBTC) || (market == BAI && other == NVDAC);
    }

    function getAmountOut(address a, address b, uint256 amount) public view returns (uint256 out) {
        if (block.chainid != 8453 || !supported(ELFOMO, a, b)) revert UnsupportedToken();
        out = IElfomoRouter(ELFOMO).getAmountOut(a, b, amount);
        if (out >= IElfomoToken(b).balanceOf(VAULT) || out >= IElfomoToken(b).allowance(VAULT, ELFOMO)) revert ExhaustedVault();
    }

    function quoteLunar(address a, address b, uint256 amount) public view returns (uint256) {
        if (block.chainid != 8453 || !supported(LUNAR, a, b)) revert UnsupportedToken();
        ILunarPool pool = ILunarPool(LUNAR);
        if (pool.paused() || !pool.isFresh()) revert ClosedMarket();
        return pool.quoteExactIn(a == WETH ? address(0) : a, b == WETH ? address(0) : b, amount);
    }

    function quoteBai(address a, address b, uint256 amount) public view returns (uint256 out) {
        if (block.chainid != 8453 || !supported(BAI,a,b)) revert UnsupportedToken();
        (out,) = IBaiEntry(BAI).quoteFor(a == USDC ? b : a, a, amount, address(this));
        if (out == 0 || out > IElfomoToken(b).balanceOf(BAI_VAULT)) revert ClosedMarket();
    }

    function execute(address market, address a, address b, uint256 amount, uint256 minimum,
                     uint256 deadline) external returns (uint256 received) {
        if (block.chainid != 8453 || trading || deadline < block.timestamp ||
            minimum == 0 || amount == 0 || amount > uint256(type(int256).max)) revert InvalidOrder();
        bool nativeV3 = market != ELFOMO && market != LUNAR && market != BAI;
        address spender = market;
        uint24 fee;
        address bonding;
        if (nativeV3) {
            (spender, fee) = BaseNativeV3.market(market, a, b);
            if (spender == address(0)) (spender,bonding)=BaseVirtuals.market(market,a,b);
            if (spender == address(0)) revert InvalidOrder();
        } else if (!supported(market, a, b)) revert UnsupportedToken();
        trading = true;
        IElfomoToken input = IElfomoToken(a);
        IElfomoToken output = IElfomoToken(b);
        uint256 beforeIn = input.balanceOf(msg.sender);
        uint256 beforeOut = output.balanceOf(msg.sender);
        uint256 heldIn = input.balanceOf(address(this));
        uint256 heldOut = output.balanceOf(address(this));
        uint256 nativeBefore = address(this).balance;
        if (input.allowance(address(this), spender) != 0) revert UnsupportedToken();
        checked(a, abi.encodeCall(input.transferFrom, (msg.sender, address(this), amount)));
        if (input.balanceOf(address(this)) != heldIn + amount) revert PartialFill();
        bool approve = market != LUNAR || a != WETH;
        if (approve) checked(a, abi.encodeCall(input.approve, (spender, amount)));
        uint256 promised;
        if (bonding!=address(0)) {
            BaseVirtuals.swap(bonding,market,a,b,amount,minimum,deadline);
            uint256 paid=output.balanceOf(address(this))-heldOut;
            if(paid<minimum) revert InsufficientOutput();
            promised = paid;
            checked(b,abi.encodeCall(IBaseWrappedEther.transfer,(msg.sender,paid)));
        } else if (nativeV3) {
            BaseNativeV3.swap(spender, fee, a, b, amount, minimum, deadline, msg.sender);
        } else if (market == ELFOMO) {
            if (getAmountOut(a, b, amount) < minimum) revert InsufficientOutput();
            IElfomoRouter(ELFOMO).swap(a, b, int256(amount), minimum, msg.sender, 0);
        } else if (market == BAI) {
            if (quoteBai(a,b,amount) < minimum) revert InsufficientOutput();
            promised = IBaiEntry(BAI).swapExactAmountIn(a == USDC ? b : a, a, amount, minimum, msg.sender);
        } else {
            if (quoteLunar(a, b, amount) < minimum) revert InsufficientOutput();
            promised = lunarSwap(a, amount, minimum, deadline, nativeBefore);
        }
        if (approve) checked(a, abi.encodeCall(input.approve, (spender, 0)));
        if (input.allowance(address(this), spender) != 0) revert UnsupportedToken();
        if (input.balanceOf(address(this)) != heldIn || output.balanceOf(address(this)) != heldOut ||
            address(this).balance != nativeBefore || beforeIn - input.balanceOf(msg.sender) != amount) revert PartialFill();
        received = output.balanceOf(msg.sender) - beforeOut;
        if (received < minimum || (!nativeV3 && market != ELFOMO && received != promised)) revert InsufficientOutput();
        trading = false;
    }

    function lunarSwap(address a, uint256 amount, uint256 minimum, uint256 deadline,
                       uint256 nativeBefore) private returns (uint256 out) {
        IBaseWrappedEther wrapped = IBaseWrappedEther(WETH);
        ILunarPool pool = ILunarPool(LUNAR);
        if (a == WETH) {
            wrapped.withdraw(amount);
            return pool.swapExactInNative{value:amount}(USDC, msg.sender, minimum, deadline);
        }
        out = pool.swapExactIn(ILunarPool.Order(USDC, address(0), address(this), amount, minimum, deadline));
        uint256 paid = address(this).balance - nativeBefore;
        if (paid != out || paid < minimum) revert InsufficientOutput();
        wrapped.deposit{value:paid}();
        checked(WETH, abi.encodeCall(wrapped.transfer, (msg.sender, paid)));
    }

    function checked(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        if (!ok || (result.length != 0 && !abi.decode(result, (bool)))) revert UnsupportedToken();
    }
}
