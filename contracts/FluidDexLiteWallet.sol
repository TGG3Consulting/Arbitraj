// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

import {IBalancerVault, IERC20} from "./IPools.sol";

interface IFluidDexLite {
    struct DexKey { address token0; address token1; bytes32 salt; }
    function readFromStorage(bytes32 slot) external view returns (uint256);
    function swapSingle(DexKey calldata pool, bool forward, int256 specified, uint256 limit,
                        address recipient, bool callback, bytes calldata data, bytes calldata extra)
        external payable returns (uint256 unspecified);
}

interface IFluidWrapped is IERC20 {
    function withdraw(uint256) external;
    function deposit() external payable;
}

// Instadapp/fluid-contracts-public@404d73f9, deployments/mainnet/FluidDexLite.json, embedded metadata.
// The core exposes the original DexKey array in slot 1. Its trading state is keyed by the first
// eight bytes of keccak256(abi.encode(DexKey)). Keep the core identity and array position together.
contract FluidDexLiteWallet {
    address private constant NATIVE = 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;
    uint256 private constant ARRAY_START = uint256(keccak256(abi.encode(uint256(1))));
    IFluidDexLite public immutable dex;
    IFluidWrapped public immutable wrapped;
    bytes32 public immutable venueHash;
    address transient private payer;
    address transient private source;
    uint256 transient private remaining;
    uint256 transient private paid;

    error Market();
    error Order();
    error Payment();
    event Filled(bytes32 indexed market, address indexed payer, address indexed recipient,
                 address input, address output, uint256 spent, uint256 received);

    constructor(IFluidDexLite core, IFluidWrapped weth) {
        if (address(core).code.length == 0 || address(weth).code.length == 0) revert Market();
        dex = core;
        wrapped = weth;
        venueHash = address(core).codehash;
    }

    receive() external payable {
        if (msg.sender != address(dex) && msg.sender != address(wrapped)) revert Payment();
    }

    function resolve(bytes32 market) public view returns (IFluidDexLite.DexKey memory k) {
        if (address(dex).codehash != venueHash) revert Market();
        uint256 index = uint192(uint256(market));
        if (index >= dex.readFromStorage(bytes32(uint256(1)))) revert Market();
        uint256 pos = ARRAY_START + index * 3;
        k = IFluidDexLite.DexKey(address(uint160(dex.readFromStorage(bytes32(pos)))),
                                 address(uint160(dex.readFromStorage(bytes32(pos+1)))),
                                 bytes32(dex.readFromStorage(bytes32(pos+2))));
        if (bytes8(keccak256(abi.encode(k))) != bytes8(market)) revert Market();
    }

    function execute(bytes32 market, address input, address output, uint256 budget, uint256 minimum,
                     bool exactOut, uint256 deadline) external returns (uint256 spent, uint256 received) {
        return settle(market, input, output, budget, minimum, exactOut, deadline, msg.sender);
    }

    function swap(IBalancerVault.SingleSwap calldata o, IBalancerVault.FundManagement calldata f,
                  uint256 minimum, uint256 deadline) external returns (uint256 received) {
        if (o.kind != 0 || o.userData.length != 0 || f.sender != msg.sender || f.fromInternalBalance ||
            f.toInternalBalance) revert Order();
        (, received) = settle(o.poolId, o.assetIn, o.assetOut, o.amount, minimum, false, deadline, f.recipient);
    }

    function asset(address physical) private view returns (address) {
        return physical == NATIVE ? address(wrapped) : physical;
    }

    function settle(bytes32 market, address input, address output, uint256 budget, uint256 minimum,
                    bool exactOut, uint256 deadline, address recipient) private returns (uint256 spent, uint256 received) {
        if (payer != address(0) || input == output || input == address(0) || output == address(0) ||
            recipient == address(0) || recipient == address(this) || block.timestamp > deadline ||
            budget == 0 || minimum == 0 || budget > uint256(type(int256).max) ||
            minimum > uint256(type(int256).max)) revert Order();
        IFluidDexLite.DexKey memory k = resolve(market);
        bool forward = input == asset(k.token0);
        address physicalIn = forward ? k.token0 : k.token1;
        address physicalOut = forward ? k.token1 : k.token0;
        if (asset(physicalIn) != input || asset(physicalOut) != output) revert Market();
        payer = msg.sender;
        source = physicalIn;
        remaining = budget;
        uint256 beforeIn = IERC20(input).balanceOf(msg.sender);
        uint256 beforeOut = IERC20(output).balanceOf(recipient);
        uint256 beforeNative = address(this).balance;
        uint256 beforeWrapped = physicalIn == NATIVE || physicalOut == NATIVE ? wrapped.balanceOf(address(this)) : 0;
        uint256 actual = dex.swapSingle(k, forward, exactOut ? -int256(minimum) : int256(budget),
                                        exactOut ? budget : minimum, physicalOut == NATIVE ? address(this) : recipient,
                                        true, "", "");
        if (physicalOut == NATIVE) {
            uint256 credit = address(this).balance - beforeNative;
            if (credit != (exactOut ? minimum : actual)) revert Payment();
            wrapped.deposit{value: credit}();
            tokenCall(address(wrapped), abi.encodeCall(IERC20.transfer, (recipient, credit)));
        }
        spent = beforeIn - IERC20(input).balanceOf(msg.sender);
        received = IERC20(output).balanceOf(recipient) - beforeOut;
        if (paid != (exactOut ? actual : budget) || spent != paid || received < minimum ||
            (exactOut && spent > budget) || address(this).balance != beforeNative ||
            ((physicalIn == NATIVE || physicalOut == NATIVE) && wrapped.balanceOf(address(this)) != beforeWrapped))
            revert Payment();
        emit Filled(market, msg.sender, recipient, input, output, spent, received);
        payer = address(0);
        source = address(0);
        remaining = 0;
        paid = 0;
    }

    // Fluid pays the output first, then calls back once to collect the precise input.
    // Transfer the ERC20 directly from the original payer to the core without a second token fee.
    function dexCallback(address token, uint256 amount, bytes calldata data) external {
        if (msg.sender != address(dex) || payer == address(0) || token != source || amount == 0 ||
            amount > remaining || paid != 0 || data.length != 0) revert Payment();
        remaining = 0;
        paid = amount;
        if (token == NATIVE) {
            tokenCall(address(wrapped), abi.encodeCall(IERC20.transferFrom, (payer, address(this), amount)));
            wrapped.withdraw(amount);
            (bool ok,) = payable(msg.sender).call{value: amount}("");
            if (!ok) revert Payment();
        } else {
            tokenCall(token, abi.encodeCall(IERC20.transferFrom, (payer, msg.sender, amount)));
        }
    }

    function tokenCall(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        if (!ok || (result.length != 0 && !abi.decode(result, (bool)))) revert Payment();
    }
}
