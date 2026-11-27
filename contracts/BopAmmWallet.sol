// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Оплата из кошелька в одном слоте. Рынок распределяет вход между маркетмейкерами.
// Плательщик получает выход напрямую, прежние остатки рынка сохраняются.
interface IBopWalletToken {
    function balanceOf(address who) external view returns (uint256);
    function transferFrom(address from, address to, uint256 value) external returns (bool);
}

interface IBopMarket {
    function pricing() external view returns (address);
    function usdc() external view returns (address);
    function paused() external view returns (bool);
    function isActive(address input, address output) external view returns (bool);
    function swap(address input, address output, uint256 amount, uint256 minimum,
                  address recipient, uint256 deadline) external returns (uint256);
}

contract BopAmmWallet {
    IBopMarket public immutable market;
    address public immutable pricing;
    address public immutable usdc;
    struct Order {
        bytes32 leg;
        bytes32 parent;
        uint64 height;
        uint64 timestamp;
        address input;
        address output;
        uint256 amount;
        uint256 minimum;
    }
    mapping(address => mapping(bytes32 => bool)) public completed;
    bool transient private busy;
    event Filled(address indexed payer, bytes32 indexed leg, address indexed input,
                 address output, uint256 spent, uint256 received);
    error InvalidMarket();
    error WrongBlock();
    error InvalidOrder();
    error InvalidFill();

    constructor(IBopMarket venue, address expectedPricing, address expectedUsdc) {
        if (block.chainid != 1 || address(venue).code.length == 0 || expectedPricing.code.length == 0 ||
            expectedUsdc.code.length == 0 || venue.pricing() != expectedPricing || venue.usdc() != expectedUsdc)
            revert InvalidMarket();
        market = venue;
        pricing = expectedPricing;
        usdc = expectedUsdc;
    }

    function execute(Order calldata o) external returns (uint256 spent, uint256 received) {
        if (o.parent == bytes32(0) || o.parent != blockhash(block.number - 1) ||
            o.height != block.number || o.timestamp != block.timestamp) revert WrongBlock();
        if (busy || o.leg == bytes32(0) || completed[msg.sender][o.leg] || o.input == o.output ||
            o.amount == 0 || o.minimum == 0 || market.paused() || !market.isActive(o.input,o.output))
            revert InvalidOrder();
        busy = true;
        completed[msg.sender][o.leg] = true;
        IBopWalletToken input = IBopWalletToken(o.input);
        IBopWalletToken output = IBopWalletToken(o.output);
        address venue = address(market);
        uint256 ownerIn = input.balanceOf(msg.sender);
        uint256 ownerOut = output.balanceOf(msg.sender);
        uint256 initialIn = input.balanceOf(venue);
        uint256 initialOut = output.balanceOf(venue);
        bool cross = usdc != o.input && usdc != o.output;
        uint256 bridge = cross ? IBopWalletToken(usdc).balanceOf(venue) : 0;
        (bool ok, bytes memory answer) = o.input.call(
            abi.encodeCall(input.transferFrom,(msg.sender,venue,o.amount)));
        if (!ok || (answer.length != 0 && (answer.length != 32 || !abi.decode(answer,(bool)))) ||
            input.balanceOf(venue) != initialIn + o.amount) revert InvalidFill();
        market.swap(o.input,o.output,o.amount,o.minimum,msg.sender,o.timestamp);
        spent = ownerIn - input.balanceOf(msg.sender);
        received = output.balanceOf(msg.sender) - ownerOut;
        if (spent != o.amount || received < o.minimum || input.balanceOf(venue) != initialIn ||
            output.balanceOf(venue) != initialOut || (cross && IBopWalletToken(usdc).balanceOf(venue) != bridge))
            revert InvalidFill();
        emit Filled(msg.sender,o.leg,o.input,o.output,spent,received);
        busy = false;
    }
}
