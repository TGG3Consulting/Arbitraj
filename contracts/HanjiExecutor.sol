// SPDX-License-Identifier: MIT
// Файл: contracts/HanjiExecutor.sol.
// Что делает: атомарный обмен на Hanji с проверкой размера остатка и фактической выручки.
// Кто вызывает: кошелёк или исполнитель маршрута, который предоставил входные токены.
// От чего зависит: прокси рынка Hanji и каноническая обёртка ETH конкретной сети.
// Настройки: при ручном размещении задаётся контракт обёртки. Пара, предел цены,
// предел комиссии, ID операции и минимальная выручка задаются каждым вызовом.
pragma solidity 0.8.37;

/// Quote and execute on the same proxy returned by Hanji's /aggregators/markets.
/// https://docs.hanji.io/for-developers/liquidity-source-integration
interface IHanjiMarket {
    struct Config {
        uint256 scaleX; uint256 scaleY; address x; address y;
        bool supportsNative; bool xIsWrapped; address askTrie; address bidTrie;
        uint64 adminFee; uint64 aggressiveFee; uint64 passiveFee; uint64 passivePayout;
        bool invokesOnTrade;
    }
    function getConfig() external view returns (Config memory);
    function placeOrder(bool,uint128,uint72,uint128,bool,bool,bool,uint256)
        external payable returns (uint64,uint128,uint128,uint128);
    function placeMarketOrderWithTargetValue(bool,uint128,uint72,uint128,bool,uint256)
        external payable returns (uint128,uint128,uint128);
}

interface IHanjiToken {
    function balanceOf(address holder) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transfer(address recipient, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}
interface IHanjiWrapped {
    function deposit() external payable;
}

/// Shared execution for a CLOB whose market-only orders may succeed with zero fills.
/// Per-call balances isolate previous donations. The caller's input, residual and output are
/// checked after transfers. Both quote-token fees and lot-size dust are included in those amounts.
contract HanjiExecutor {
    address public wrapped;
    bool private trading;
    // Stable ID of one economic leg. A reverted transaction rolls this write back, so the
    // original leg can still be retried after its revert is confirmed by the sender.
    mapping(address => mapping(bytes32 => bool)) public completed;

    struct Order {
        address market;
        address input;
        address output;
        uint256 budget;
        uint256 maxUnspent;
        uint256 minReceived;
        uint72 limitPrice;
        uint128 maxCommission;
        uint256 deadline;
        bytes32 operationId;
    }
    error InvalidOrder();
    error TransferFailed();
    error Unfilled();
    error AlreadyCompleted(bytes32 operationId);
    error UnexpectedNative();

    constructor(address wrappedToken) {
        if (wrappedToken.code.length == 0) revert InvalidOrder();
        wrapped = wrappedToken;
    }

    receive() external payable {
        if (!trading) revert UnexpectedNative();
    }

    function execute(Order calldata order) external returns (uint256 spent, uint256 received) {
        address payer = msg.sender;
        if (trading || block.timestamp > order.deadline ||
            order.operationId == bytes32(0) || order.market.code.length == 0 || order.input == order.output ||
            order.budget == 0 || order.maxUnspent >= order.budget ||
            order.minReceived == 0 || order.limitPrice == 0) revert InvalidOrder();
        if (completed[payer][order.operationId]) revert AlreadyCompleted(order.operationId);
        completed[payer][order.operationId] = true;
        trading = true;
        IHanjiMarket market = IHanjiMarket(order.market);
        IHanjiMarket.Config memory config = market.getConfig();
        bool sell = order.input == config.x && order.output == config.y;
        if (!sell && !(order.input == config.y && order.output == config.x)) revert InvalidOrder();
        uint256 scale = sell ? config.scaleX : config.scaleY;
        if (scale == 0 || order.budget / scale == 0 || order.budget / scale > type(uint128).max)
            revert InvalidOrder();
        uint128 lots = uint128(order.budget / scale);
        IHanjiToken input = IHanjiToken(order.input);
        IHanjiToken output = IHanjiToken(order.output);
        uint256 originalIn = input.balanceOf(address(this));
        uint256 originalOut = output.balanceOf(address(this));
        uint256 payerIn = input.balanceOf(payer);
        uint256 payerOut = output.balanceOf(payer);
        uint256 originalEth = address(this).balance;
        checkedCall(order.input, abi.encodeCall(input.transferFrom, (payer, address(this), order.budget)));
        if (input.balanceOf(address(this)) != originalIn + order.budget) revert TransferFailed();
        checkedCall(order.input, abi.encodeCall(input.approve, (order.market, order.budget)));
        if (sell) {
            market.placeOrder(true, lots, order.limitPrice, order.maxCommission, true, false, true, order.deadline);
        } else {
            market.placeMarketOrderWithTargetValue(false, lots, order.limitPrice, order.maxCommission, true, order.deadline);
        }
        checkedCall(order.input, abi.encodeCall(input.approve, (order.market, 0)));
        uint256 nativeOut = address(this).balance - originalEth;
        if (nativeOut != 0) {
            if (wrapped != order.input && wrapped != order.output) revert UnexpectedNative();
            IHanjiWrapped(wrapped).deposit{value: nativeOut}();
        }
        uint256 leftover = input.balanceOf(address(this)) - originalIn;
        if (leftover > order.maxUnspent) revert Unfilled();
        uint256 proceeds = output.balanceOf(address(this)) - originalOut;
        if (proceeds < order.minReceived) revert Unfilled();
        if (leftover != 0) checkedCall(order.input, abi.encodeCall(input.transfer, (payer, leftover)));
        checkedCall(order.output, abi.encodeCall(output.transfer, (payer, proceeds)));
        spent = payerIn - input.balanceOf(payer);
        received = output.balanceOf(payer) - payerOut;
        if (spent != order.budget - leftover || received < order.minReceived ||
            input.balanceOf(address(this)) != originalIn || output.balanceOf(address(this)) != originalOut ||
            address(this).balance != originalEth) revert Unfilled();
        trading = false;
    }

    function checkedCall(address token, bytes memory data) private {
        (bool success, bytes memory result) = token.call(data);
        if (!success || (result.length != 0 && !abi.decode(result,(bool)))) revert TransferFailed();
    }
}
