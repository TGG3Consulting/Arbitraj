// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Расчёт из собственного ERC20-инвентаря по одной подписанной заявке Rubicon.
// Размещение и разрешение на расход определяет владелец кошелька.
interface IGladiusInventoryToken {
    function balanceOf(address owner) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
    function approve(address spender, uint256 value) external returns (bool);
    function transfer(address to, uint256 value) external returns (bool);
    function transferFrom(address from, address to, uint256 value) external returns (bool);
}
interface IGladiusInventoryReactor {
    struct SignedOrder { bytes order; bytes sig; }
    function execute(SignedOrder calldata order, uint256 quantity) external payable;
}

contract GladiusInventory {
    error Payment();
    address public immutable reactor;
    bytes32 public immutable reactorHash;
    bool transient private locked;

    struct Limit {
        address input;
        address output;
        uint256 minimumDebit;
        uint256 maximumDebit;
        uint256 minimumCredit;
        uint256 deadline;
    }
    event Settled(address indexed payer, bytes32 indexed orderDigest, address indexed input,
                  address output, uint256 spent, uint256 received);
    constructor(address target, bytes32 expectedHash) {
        if (target.code.length == 0 || target.codehash != expectedHash) revert Payment();
        reactor = target;
        reactorHash = expectedHash;
    }

    function fill(Limit calldata l, IGladiusInventoryReactor.SignedOrder calldata signed, uint256 quantity)
        external returns (uint256 spent, uint256 received)
    {
        if (locked || reactor.codehash != reactorHash || l.input == l.output || l.input.code.length == 0
            || l.output.code.length == 0 || block.timestamp > l.deadline || quantity == 0
            || l.minimumCredit == 0 || l.maximumDebit == 0 || l.minimumDebit > l.maximumDebit) revert Payment();
        locked = true;
        IGladiusInventoryToken src = IGladiusInventoryToken(l.input);
        IGladiusInventoryToken dst = IGladiusInventoryToken(l.output);
        uint256 beforeIn = src.balanceOf(msg.sender);
        uint256 beforeOut = dst.balanceOf(msg.sender);
        uint256 heldIn = src.balanceOf(address(this));
        uint256 heldOut = dst.balanceOf(address(this));
        if (src.allowance(address(this), reactor) != 0) revert Payment();
        tokenCall(l.input, abi.encodeCall(src.transferFrom, (msg.sender, address(this), l.maximumDebit)));
        if (src.balanceOf(address(this)) != heldIn + l.maximumDebit) revert Payment();
        tokenCall(l.input, abi.encodeCall(src.approve, (reactor, l.maximumDebit)));
        IGladiusInventoryReactor(reactor).execute(signed, quantity);
        if (src.allowance(address(this), reactor) != 0) tokenCall(l.input, abi.encodeCall(src.approve, (reactor, 0)));
        uint256 unused = src.balanceOf(address(this)) - heldIn;
        if (unused > l.maximumDebit) revert Payment();
        if (unused != 0) tokenCall(l.input, abi.encodeCall(src.transfer, (msg.sender, unused)));
        uint256 proceeds = dst.balanceOf(address(this)) - heldOut;
        if (proceeds == 0) revert Payment();
        tokenCall(l.output, abi.encodeCall(dst.transfer, (msg.sender, proceeds)));
        spent = beforeIn - src.balanceOf(msg.sender);
        received = dst.balanceOf(msg.sender) - beforeOut;
        if (spent == 0 || spent != l.maximumDebit - unused || spent < l.minimumDebit
            || received < l.minimumCredit || src.balanceOf(address(this)) != heldIn
            || dst.balanceOf(address(this)) != heldOut || src.allowance(address(this), reactor) != 0) revert Payment();
        emit Settled(msg.sender, keccak256(signed.order), l.input, l.output, spent, received);
        locked = false;
    }

    function tokenCall(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        if (!ok || (result.length != 0 && !abi.decode(result, (bool)))) revert Payment();
    }
}
