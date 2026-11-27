// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Расчёт немедленной котировки Textile своим кошельком. Авторизованный taker заявки — этот модуль.
// Для выдачи фирменной котировки на такой адрес оператору требуется поддерживаемый Textile доступ.
interface ITextileToken {
    function balanceOf(address) external view returns (uint256);
    function allowance(address,address) external view returns (uint256);
    function approve(address,uint256) external returns (bool);
    function transfer(address,uint256) external returns (bool);
    function transferFrom(address,address,uint256) external returns (bool);
}

interface ITextileReactor {
    struct SignedOrder { bytes order; bytes sig; }
    function execute(SignedOrder calldata) external payable;
    function executeBatch(SignedOrder[] calldata) external payable;
}

contract TextileWallet {
    address public immutable owner;
    address public immutable reactor;
    bytes32 public immutable reactorCodeHash;
    bool transient private entered;

    error InvalidOrder();
    error Payment();
    event Settled(address indexed payer, bytes32 indexed action, address indexed sellToken,
                  address buyToken, uint256 spent, uint256 received);

    constructor(address owner_, address reactor_, bytes32 expectedCodeHash) {
        if (owner_ == address(0) || reactor_.code.length == 0 || reactor_.codehash != expectedCodeHash) revert InvalidOrder();
        owner = owner_;
        reactor = reactor_;
        reactorCodeHash = expectedCodeHash;
    }

    struct Terms {
        ITextileToken sell;
        ITextileToken buy;
        uint256 takerPays;
        uint256 minimum;
        uint256 deadline;
    }

    function settle(Terms calldata t, bytes calldata action) external returns (uint256 received) {
        if (msg.sender != owner || entered || reactor.codehash != reactorCodeHash || block.timestamp > t.deadline ||
            address(t.sell) == address(t.buy) || t.takerPays == 0 || t.minimum == 0 || action.length < 4) revert InvalidOrder();
        bytes4 selector = bytes4(action[:4]);
        if (selector != ITextileReactor.execute.selector && selector != ITextileReactor.executeBatch.selector) revert InvalidOrder();
        entered = true;
        uint256 ownerIn = t.sell.balanceOf(owner);
        uint256 ownerOut = t.buy.balanceOf(owner);
        uint256 heldIn = t.sell.balanceOf(address(this));
        uint256 heldOut = t.buy.balanceOf(address(this));
        if (t.sell.allowance(address(this),reactor) != 0) revert Payment();
        tokenCall(address(t.sell),abi.encodeCall(t.sell.transferFrom,(owner,address(this),t.takerPays)));
        if (t.sell.balanceOf(address(this)) - heldIn != t.takerPays) revert Payment();
        tokenCall(address(t.sell),abi.encodeCall(t.sell.approve,(reactor,t.takerPays)));
        (bool ok,) = reactor.call(action);
        if (!ok) revert Payment();
        if (t.sell.allowance(address(this),reactor) != 0) tokenCall(address(t.sell),abi.encodeCall(t.sell.approve,(reactor,0)));
        uint256 amount = t.buy.balanceOf(address(this)) - heldOut;
        if (amount < t.minimum) revert Payment();
        tokenCall(address(t.buy),abi.encodeCall(t.buy.transfer,(owner,amount)));
        received = t.buy.balanceOf(owner) - ownerOut;
        if (received < t.minimum || ownerIn - t.sell.balanceOf(owner) != t.takerPays ||
            t.sell.balanceOf(address(this)) != heldIn || t.buy.balanceOf(address(this)) != heldOut ||
            t.sell.allowance(address(this),reactor) != 0) revert Payment();
        entered = false;
        emit Settled(owner,keccak256(action),address(t.sell),address(t.buy),t.takerPays,received);
    }

    function tokenCall(address token,bytes memory payload) private {
        (bool ok, bytes memory result) = token.call(payload);
        if (!ok || (result.length != 0 && !abi.decode(result,(bool)))) revert Payment();
    }
}
