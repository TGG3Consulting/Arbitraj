// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface IUniV1Factory {
    function getExchange(address token) external view returns (address);
    function getToken(address exchange) external view returns (address);
}
interface IUniV1Exchange {
    function ethToTokenTransferInput(uint256 minimum, uint256 deadline, address recipient) external payable returns (uint256);
    function tokenToTokenTransferInput(uint256 amount,uint256 minTokens,uint256 minETH,uint256 deadline,address recipient,address token) external returns(uint256);
    function tokenToEthSwapInput(uint256 amount, uint256 minimum, uint256 deadline) external returns (uint256);
}
interface IUniV1Asset {
    function balanceOf(address account) external view returns (uint256);
    function transferFrom(address owner, address recipient, uint256 amount) external returns (bool);
    function approve(address spender, uint256 amount) external returns (bool);
    function allowance(address owner, address spender) external view returns (uint256);
    function transfer(address recipient, uint256 amount) external returns (bool);
    function withdraw(uint256 amount) external;
    function deposit() external payable;
}

/// Holds ETH only between the original V1 exchange and its wrapped equivalent in the same call.
contract UniswapV1Adapter {
    IUniV1Factory public immutable factory;
    IUniV1Asset public immutable wrapped;
    bool transient private entered;
    event Filled(address indexed payer, address indexed exchange, address indexed recipient,
                 address assetIn, address assetOut, uint256 paid, uint256 received);

    constructor(IUniV1Factory registry, IUniV1Asset weth) {
        require(address(registry).code.length != 0 && address(weth).code.length != 0 && address(registry) != address(weth), "deployment");
        factory = registry;
        wrapped = weth;
    }

    // V1 pays the seller with Vyper send (2300 gas).
    receive() external payable {}

    function execute(address pool, address tokenIn, address tokenOut, uint256 amount, uint256 minimum, uint256 deadline)
        external returns (uint256) {
        return settle(pool, tokenIn, tokenOut, amount, minimum, deadline, msg.sender, address(0));
    }

    function executePair(address pool,address targetPool,address tokenIn,address tokenOut,uint256 amount,uint256 minimum,uint256 deadline)
        external returns(uint256) {
        require(targetPool != address(0), "target market");
        return settle(pool,tokenIn,tokenOut,amount,minimum,deadline,msg.sender,targetPool);
    }

    function swap(IBalancerVault.SingleSwap calldata order, IBalancerVault.FundManagement calldata funds,
                  uint256 minimum, uint256 deadline) external returns (uint256) {
        require(order.kind == 0 && uint256(order.poolId) >> 160 == 0 && order.userData.length == 0 &&
                funds.sender == msg.sender && !funds.fromInternalBalance && !funds.toInternalBalance, "order");
        address market = address(uint160(uint256(order.poolId)));
        address second;
        if (market == address(factory)) {
            market = factory.getExchange(order.assetIn);
            second = factory.getExchange(order.assetOut);
            require(market != address(0) && second != address(0), "unknown market");
        }
        return settle(market, order.assetIn, order.assetOut, order.amount, minimum, deadline, funds.recipient, second);
    }

    function callToken(IUniV1Asset asset, bytes memory data) private {
        (bool ok, bytes memory result) = address(asset).call(data);
        require(ok && (result.length == 0 || abi.decode(result,(bool))), "token");
    }

    struct Balances {
        uint256 payerIn;
        uint256 recipientOut;
        uint256 moduleIn;
        uint256 moduleETH;
    }

    function sellIntoPool(address pool, IUniV1Asset source, address tokenOut, uint256 amount,
                          uint256 minimum, uint256 deadline, address recipient, bool toETH) private {
        uint256 initialWrapped = wrapped.balanceOf(address(this));
        callToken(source, abi.encodeCall(source.approve,(pool,amount)));
        if (toETH) {
            uint256 beforeETH = address(this).balance;
            uint256 gross = IUniV1Exchange(pool).tokenToEthSwapInput(amount,minimum,deadline);
            require(address(this).balance - beforeETH == gross, "native output");
            wrapped.deposit{value:gross}();
            callToken(wrapped, abi.encodeCall(wrapped.transfer,(recipient,gross)));
        } else {
            IUniV1Exchange(pool).tokenToTokenTransferInput(amount,minimum,1,deadline,recipient,tokenOut);
        }
        if (source.allowance(address(this),pool) != 0) {
            callToken(source, abi.encodeCall(source.approve,(pool,0)));
            require(source.allowance(address(this),pool) == 0, "allowance");
        }
        require(wrapped.balanceOf(address(this)) == initialWrapped - (source == wrapped ? amount : 0), "wrapped residue");
    }

    function settle(address pool, address tokenIn, address tokenOut, uint256 amount, uint256 minimum,
                    uint256 deadline, address recipient, address targetPool) private returns (uint256 received) {
        require(!entered && amount > 0 && minimum > 0 && block.timestamp <= deadline &&
                tokenIn != tokenOut && recipient != address(0) && recipient != address(this) && recipient != pool, "order");
        address asset = factory.getToken(pool);
        require(asset != address(0) && factory.getExchange(asset) == pool, "market");
        bool buy = targetPool == address(0) && tokenIn == address(wrapped) && asset == tokenOut;
        bool sell = targetPool == address(0) && tokenOut == address(wrapped) && asset == tokenIn;
        if (!buy && !sell) require(asset == tokenIn && targetPool != address(0) && targetPool != pool &&
                                  factory.getExchange(tokenOut) == targetPool && factory.getToken(targetPool) == tokenOut, "target market");
        entered = true;
        IUniV1Asset source = IUniV1Asset(tokenIn);
        IUniV1Asset target = IUniV1Asset(tokenOut);
        Balances memory beforeSwap = Balances(source.balanceOf(msg.sender), target.balanceOf(recipient),
                                              source.balanceOf(address(this)), address(this).balance);
        callToken(source, abi.encodeCall(source.transferFrom,(msg.sender,address(this),amount)));
        require(source.balanceOf(address(this)) == beforeSwap.moduleIn + amount, "partial input");
        if (buy) {
            wrapped.withdraw(amount);
            IUniV1Exchange(pool).ethToTokenTransferInput{value:amount}(minimum,deadline,recipient);
        } else {
            sellIntoPool(pool,source,tokenOut,amount,minimum,deadline,recipient,sell);
        }
        received = target.balanceOf(recipient) - beforeSwap.recipientOut;
        require(received >= minimum && beforeSwap.payerIn - source.balanceOf(msg.sender) == amount &&
                source.balanceOf(address(this)) == beforeSwap.moduleIn && address(this).balance == beforeSwap.moduleETH, "wallet bounds");
        emit Filled(msg.sender,pool,recipient,tokenIn,tokenOut,amount,received);
        entered = false;
    }
}
