// SPDX-License-Identifier: MIT
// Прямой расчёт Native RFQ v6. Владелец размещает модуль с проверенными Router и RFQPool.
// Конечный получатель и плательщик совпадают. Котировка живёт до подписанного срока.
pragma solidity 0.8.37;

interface INativeRfqToken {
    function allowance(address account, address spender) external view returns (uint256);
    function balanceOf(address account) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

contract NativeRfqAdapter {
    address public immutable router;
    address public immutable pool;
    bytes32 public immutable routerHash;
    bytes32 public immutable poolHash;
    bool transient private busy;

    // Одна наблюдавшаяся ABI-форма tradeRFQT v6, с двумя подписями по 65 байт.
    // Внешние overrides нулевые. Владелец передаёт неизменённые байты исходного предложения.
    struct Order {
        address market;
        address maker;
        address recipient;
        address input;
        address output;
        uint256 sell;
        uint256 buy;
        uint256 minimum;
        uint256 expiry;
        uint256 nonce;
        uint256[10] confidence;
        bytes16 quoteId;
        bool multiHop;
        uint256 mode;
        bytes signature;
        address feeRecipient;
        uint256 feeRate;
        bytes feeSignature;
    }

    error WrongMarket();
    error WrongOffer();
    error WrongPayment();

    event Settled(bytes16 indexed quoteId, address indexed payer, address indexed maker,
        bytes32 offerDigest, address input, address output, uint256 spent, uint256 received);

    constructor(address target, bytes32 targetHash, address market, bytes32 marketHash) {
        if (target == market || target.code.length == 0 || market.code.length == 0
            || target.codehash != targetHash || market.codehash != marketHash) revert WrongMarket();
        router = target;
        pool = market;
        routerHash = targetHash;
        poolHash = marketHash;
    }

    function execute(address input, address output, uint256 amount, uint256 minimum,
                     uint256 deadline, bytes calldata offer) external returns (uint256 received) {
        if (busy || input == output || input.code.length == 0 || output.code.length == 0
            || amount == 0 || minimum == 0 || block.timestamp > deadline) revert WrongOffer();
        if (router.codehash != routerHash || pool.codehash != poolHash) revert WrongMarket();
        if (offer.length != 1220 || bytes4(offer[:4]) != 0x7083527c) revert WrongOffer();
        (Order memory order, uint256 inputOverride, uint256 outputOverride) =
            abi.decode(offer[4:], (Order, uint256, uint256));
        if (inputOverride != 0 || outputOverride != 0 || order.input != input || order.output != output
            || order.market != pool || order.recipient != msg.sender || order.maker == address(0)
            || order.sell != amount || order.minimum == 0 || order.buy < order.minimum || block.timestamp >= order.expiry
            || order.quoteId == bytes16(0) || order.multiHop || order.mode != 1 || order.feeRate != 0
            || order.signature.length != 65 || order.feeSignature.length != 65
            || order.confidence[6] != 0 || order.confidence[7] != 0
            || keccak256(offer[4:]) != keccak256(abi.encode(order, inputOverride, outputOverride))) revert WrongOffer();

        busy = true;
        INativeRfqToken src = INativeRfqToken(input);
        INativeRfqToken dst = INativeRfqToken(output);
        uint256 beforeIn = src.balanceOf(msg.sender);
        uint256 beforeOut = dst.balanceOf(msg.sender);
        uint256 ownIn = src.balanceOf(address(this));
        uint256 ownOut = dst.balanceOf(address(this));
        if (src.allowance(address(this), router) != 0) revert WrongPayment();
        tokenCall(input, abi.encodeCall(src.transferFrom, (msg.sender, address(this), amount)));
        if (src.balanceOf(address(this)) - ownIn != amount) revert WrongPayment();
        tokenCall(input, abi.encodeCall(src.approve, (router, amount)));
        (bool success, ) = router.call(offer);
        if (!success) revert WrongOffer();
        if (src.allowance(address(this), router) != 0) tokenCall(input, abi.encodeCall(src.approve, (router, 0)));
        received = dst.balanceOf(msg.sender) - beforeOut;
        if (received < minimum || received < order.minimum || beforeIn - src.balanceOf(msg.sender) != amount
            || src.balanceOf(address(this)) != ownIn || dst.balanceOf(address(this)) != ownOut
            || src.allowance(address(this), router) != 0) revert WrongPayment();
        emit Settled(order.quoteId, msg.sender, order.maker, keccak256(offer), input, output, amount, received);
        busy = false;
    }

    function tokenCall(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        if (!ok || (result.length != 0 && !abi.decode(result, (bool)))) revert WrongPayment();
    }
}
