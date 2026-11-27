// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

interface IRubiconOrders {
    function getOffer(uint256 id) external view returns (uint256, address, uint256, address);
    function buy(uint256 id, uint256 quantity) external returns (bool);
}

interface IRubiconCash {
    function balanceOf(address owner) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transfer(address recipient, uint256 amount) external returns (bool);
    function transferFrom(address owner, address recipient, uint256 amount) external returns (bool);
}

/// Uses only the caller's funds. Each action fixes the maker order and output amount.
contract RubiconClassicWallet {
    IRubiconOrders public immutable market;
    bytes32 public immutable marketCodeHash;
    bool transient private entered;

    struct Take {
        uint256 id;
        uint128 quantity;
    }

    event Settled(address indexed payer, address indexed source, address indexed target,
        bytes32 actionsHash, uint256 spent, uint256 received);

    constructor(IRubiconOrders venue) {
        require(address(venue).code.length != 0, "market");
        market = venue;
        marketCodeHash = address(venue).codehash;
    }

    function tokenCall(IRubiconCash token, bytes memory payload) private {
        (bool ok, bytes memory result) = address(token).call(payload);
        require(ok && (result.length == 0 || abi.decode(result, (bool))), "token transfer");
    }

    function execute(IRubiconCash source, IRubiconCash target, Take[] calldata actions,
        uint256 maximum, uint256 minimum, uint256 deadline) external returns (uint256 spent, uint256 received) {
        require(!entered && block.timestamp <= deadline && source != target, "stale or invalid");
        require(address(market).codehash == marketCodeHash, "market code changed");
        require(actions.length > 0 && maximum > 0 && minimum > 0, "empty");
        entered = true;
        uint256 walletIn = source.balanceOf(msg.sender);
        uint256 walletOut = target.balanceOf(msg.sender);
        uint256 reserveIn = source.balanceOf(address(this));
        uint256 reserveOut = target.balanceOf(address(this));
        tokenCall(source, abi.encodeCall(source.transferFrom,(msg.sender,address(this),maximum)));
        require(source.balanceOf(address(this)) == reserveIn + maximum, "funding");
        tokenCall(source, abi.encodeCall(source.approve,(address(market),maximum)));

        for (uint256 i; i < actions.length; ++i) {
            Take calldata take = actions[i];
            (uint256 available, address sellToken,, address buyToken) = market.getOffer(take.id);
            require(sellToken == address(target) && buyToken == address(source) &&
                take.quantity != 0 && take.quantity <= available, "order changed");
            require(market.buy(take.id, take.quantity), "unfilled");
        }

        if (source.allowance(address(this), address(market)) != 0) {
            tokenCall(source, abi.encodeCall(source.approve,(address(market),0)));
            require(source.allowance(address(this), address(market)) == 0, "remaining allowance");
        }
        uint256 refund = source.balanceOf(address(this)) - reserveIn;
        if (refund != 0) tokenCall(source, abi.encodeCall(source.transfer,(msg.sender,refund)));
        uint256 gross = target.balanceOf(address(this)) - reserveOut;
        tokenCall(target, abi.encodeCall(target.transfer,(msg.sender,gross)));
        spent = walletIn - source.balanceOf(msg.sender);
        received = target.balanceOf(msg.sender) - walletOut;
        require(spent > 0 && spent <= maximum && received >= minimum, "wallet bounds");
        require(source.balanceOf(address(this)) == reserveIn && target.balanceOf(address(this)) == reserveOut, "inventory");
        emit Settled(msg.sender,address(source),address(target),keccak256(abi.encode(actions)),spent,received);
        entered = false;
    }
}
