// SPDX-License-Identifier: MIT
// Защищает обе дельты плательщика при расчёте зарегистрированным 0x Settler через AllowanceHolder.
// ABI AllowanceHolder и Settler: github.com/0xProject/0x-settler.
pragma solidity 0.8.37;

interface IZeroExToken {
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
    function approve(address spender, uint256 value) external returns (bool);
    function transferFrom(address owner, address to, uint256 value) external returns (bool);
}
interface IZeroExRegistry {
    function ownerOf(uint256 id) external view returns (address);
    function prev(uint128 feature) external view returns (address);
}
interface IZeroExHolder {
    function exec(address operator, address token, uint256 amount, address target, bytes calldata data)
        external payable returns (bytes memory);
}
interface IZeroExSettler {
    struct Slippage { address recipient; address buyToken; uint256 minAmountOut; }
    function execute(Slippage calldata slippage, bytes[] calldata actions, bytes32 zid) external payable returns (bool);
}

contract ZeroExWallet {
    address public immutable holder;
    bytes32 public immutable holderHash;
    address public immutable registry;
    bytes32 public immutable registryHash;
    bool transient private entered;
    uint128 private constant TAKER_FEATURE = 2;
    error InvalidRoute();
    error InvalidPayment();
    event Settled(address indexed payer, bytes32 indexed actionHash, address indexed settler,
                  address input, address output, uint256 debit, uint256 credit);

    constructor(address holder_, bytes32 holderHash_, address registry_, bytes32 registryHash_) {
        if (holder_ == registry_ || holder_.code.length == 0 || registry_.code.length == 0
            || holder_.codehash != holderHash_ || registry_.codehash != registryHash_) revert InvalidRoute();
        holder = holder_;
        registry = registry_;
        holderHash = holderHash_;
        registryHash = registryHash_;
    }

    function settle(address input, address output, uint256 amount, uint256 minimum,
                    uint256 expiry, bytes calldata original) external returns (uint256 received) {
        if (entered || input == output || input.code.length == 0 || output.code.length == 0
            || amount == 0 || minimum == 0 || block.timestamp > expiry || original.length < 4
            || holder.codehash != holderHash || registry.codehash != registryHash
            || bytes4(original[:4]) != IZeroExHolder.exec.selector) revert InvalidRoute();
        (address operator, address token, uint256 sell, address target, bytes memory swap) =
            abi.decode(original[4:], (address, address, uint256, address, bytes));
        if (target == address(0) || target != operator || token != input || sell != amount
            || swap.length < 4 || bytes4(swap) != IZeroExSettler.execute.selector) revert InvalidRoute();
        address active = IZeroExRegistry(registry).ownerOf(TAKER_FEATURE);
        if (active == address(0) || (active != target && IZeroExRegistry(registry).prev(TAKER_FEATURE) != target))
            revert InvalidRoute();
        // Первые три аргумента execute — статический кортеж AllowedSlippage.
        (address recipient, address buyToken, uint256 boughtAtLeast) = slippage(swap);
        if (recipient != msg.sender || buyToken != output || boughtAtLeast < minimum) revert InvalidRoute();
        entered = true;
        IZeroExToken src = IZeroExToken(input);
        IZeroExToken dst = IZeroExToken(output);
        uint256 fromBefore = src.balanceOf(msg.sender);
        uint256 toBefore = dst.balanceOf(msg.sender);
        uint256 inventory = src.balanceOf(address(this));
        uint256 heldOut = dst.balanceOf(address(this));
        if (src.allowance(address(this), holder) != 0) revert InvalidPayment();
        tokenCall(input, abi.encodeCall(src.transferFrom, (msg.sender, address(this), amount)));
        if (src.balanceOf(address(this)) - inventory != amount) revert InvalidPayment();
        tokenCall(input, abi.encodeCall(src.approve, (holder, amount)));
        (bool ok,) = holder.call(original);
        if (!ok) revert InvalidPayment();
        if (src.allowance(address(this), holder) != 0) tokenCall(input, abi.encodeCall(src.approve, (holder, 0)));
        received = dst.balanceOf(msg.sender) - toBefore;
        if (received < minimum || fromBefore - src.balanceOf(msg.sender) != amount
            || src.balanceOf(address(this)) != inventory || dst.balanceOf(address(this)) != heldOut
            || src.allowance(address(this), holder) != 0) revert InvalidPayment();
        entered = false;
        emit Settled(msg.sender, keccak256(original), target, input, output, amount, received);
    }

    function slippage(bytes memory swap) private pure returns (address, address, uint256) {
        if (swap.length < 100) revert InvalidRoute();
        uint256 who;
        uint256 coin;
        uint256 minimum;
        assembly ("memory-safe") {
            who := mload(add(swap, 36))
            coin := mload(add(swap, 68))
            minimum := mload(add(swap, 100))
        }
        if (who > type(uint160).max || coin > type(uint160).max) revert InvalidRoute();
        return (address(uint160(who)), address(uint160(coin)), minimum);
    }

    function tokenCall(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        if (!ok || (result.length != 0 && !abi.decode(result, (bool)))) revert InvalidPayment();
    }
}
