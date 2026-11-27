// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

import {IBalancerVault, IERC20} from "./IPools.sol";

interface IFluidDexLite {
    struct DexKey { address token0; address token1; bytes32 salt; }
    struct TransferParams { address to; bool isCallback; bytes callbackData; bytes extraData; }
    function swapHop(address[] calldata path, DexKey[] calldata keys, int256 specified, uint256[] calldata limits,
                     TransferParams calldata transfer) external payable returns (uint256 amount);
    function readFromStorage(bytes32 slot) external view returns (uint256);
    function swapSingle(DexKey calldata pool, bool forward, int256 specified, uint256 limit,
                        address recipient, bool callback, bytes calldata data, bytes calldata extra)
        external payable returns (uint256 unspecified);
}

interface IFluidWrapped is IERC20 {
    function withdraw(uint256) external;
    function deposit() external payable;
}

// Модуль для прямого расчёта и V5 проверяет исходный DexKey в каталоге Fluid DEX Lite.
contract FluidDexLiteWallet {
    address private constant NATIVE = 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;
    // _dexesList: contracts/protocols/dexLite/other/variables.sol:11 и storageLayout основной кассы.
    // https://github.com/Instadapp/fluid-contracts-public/blob/404d73f91e6806f881f8dabeb30494f93f9e99b7/deployments/mainnet/FluidDexLite.json#L550-L556
    uint256 private constant DEX_KEYS_SLOT = 1;
    uint256 private constant ARRAY_START = uint256(keccak256(abi.encode(DEX_KEYS_SLOT)));
    // DexKey(token0, token1, salt): три слова по 32 байта, поля в слотах 0, 1 и 2.
    // https://github.com/Instadapp/fluid-contracts-public/blob/404d73f91e6806f881f8dabeb30494f93f9e99b7/deployments/mainnet/FluidDexLite.json#L627-L656
    uint256 private constant DEX_KEY_WORDS = 3;
    uint256 private constant TOKEN1_OFFSET = 1;
    uint256 private constant SALT_OFFSET = 2;
    // Fluid: metadata.sources["contracts/protocols/dexLite/core/coreInternals.sol"], строки 12 и 153:
    // dexId = bytes8(keccak256(abi.encode(dexKey))). PoolRef нашего адаптера дописывает индекс в младшие 192 бита.
    // https://github.com/Instadapp/fluid-contracts-public/blob/404d73f91e6806f881f8dabeb30494f93f9e99b7/deployments/mainnet/FluidDexLite.json#L495
    uint256 private constant MARKET_INDEX_MASK = type(uint192).max;
    bytes32 private constant DEX_ID_MASK = ~bytes32(MARKET_INDEX_MASK);
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

    event HopFilled(bytes32 indexed route, address indexed payer, address indexed recipient,
                    address input, address output, uint256 spent, uint256 received);

    struct Balances {
        uint256 input;
        uint256 output;
        uint256 nativeAmount;
        uint256 wrappedAmount;
        bool usesNative;
    }

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
        uint256 index = uint256(market) & MARKET_INDEX_MASK;
        if (index >= dex.readFromStorage(bytes32(DEX_KEYS_SLOT))) revert Market();
        uint256 pos = ARRAY_START + index * DEX_KEY_WORDS;
        k = IFluidDexLite.DexKey(address(uint160(dex.readFromStorage(bytes32(pos)))),
                                 address(uint160(dex.readFromStorage(bytes32(pos + TOKEN1_OFFSET)))),
                                 bytes32(dex.readFromStorage(bytes32(pos + SALT_OFFSET))));
        if ((keccak256(abi.encode(k)) & DEX_ID_MASK) != (market & DEX_ID_MASK)) revert Market();
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
        IFluidDexLite.DexKey memory k = resolve(market);
        bool forward = input == asset(k.token0);
        address physicalIn = forward ? k.token0 : k.token1;
        address physicalOut = forward ? k.token1 : k.token0;
        if (asset(physicalIn) != input || asset(physicalOut) != output) revert Market();
        Balances memory before = begin(input, output, physicalIn, physicalOut, budget, minimum, deadline, recipient);
        uint256 actual = dex.swapSingle(k, forward, exactOut ? -int256(minimum) : int256(budget),
                                        exactOut ? budget : minimum, physicalOut == NATIVE ? address(this) : recipient,
                                        true, "", "");
        (spent, received) = finish(before, input, output, physicalOut, recipient,
                                   exactOut ? actual : budget, exactOut ? minimum : actual, minimum);
        emit Filled(market, msg.sender, recipient, input, output, spent, received);
    }

    function hopVersion() external pure returns (uint256) { return 1; }

    function executeHop(bytes32[] calldata markets, address[] calldata path, uint256 amountIn,
                        uint256 minimum, uint256 deadline) external returns (uint256 spent, uint256 received) {
        if (markets.length < 2 || path.length != markets.length + 1) revert Order();
        address[] memory physical = new address[](path.length);
        IFluidDexLite.DexKey[] memory keys = new IFluidDexLite.DexKey[](markets.length);
        for (uint256 i; i < markets.length; ++i) {
            keys[i] = resolve(markets[i]);
            bool forward = asset(keys[i].token0) == path[i];
            address from = forward ? keys[i].token0 : keys[i].token1;
            address to = forward ? keys[i].token1 : keys[i].token0;
            if (path[i] == path[i+1] || asset(from) != path[i] || asset(to) != path[i+1] ||
                (i > 0 && physical[i] != from)) revert Market();
            physical[i] = from;
            physical[i+1] = to;
        }
        uint256 last = markets.length;
        Balances memory before = begin(path[0], path[last], physical[0], physical[last], amountIn, minimum, deadline, msg.sender);
        uint256[] memory limits = new uint256[](last);
        limits[last-1] = minimum;
        IFluidDexLite.TransferParams memory fund = IFluidDexLite.TransferParams(
            physical[last] == NATIVE ? address(this) : msg.sender, true, "", "");
        uint256 out = dex.swapHop(physical, keys, int256(amountIn), limits, fund);
        (spent, received) = finish(before, path[0], path[last], physical[last], msg.sender, amountIn, out, minimum);
        emit HopFilled(keccak256(abi.encode(markets,path)), msg.sender, msg.sender, path[0], path[last], spent, received);
    }

    function begin(address input, address output, address physicalIn, address physicalOut, uint256 budget,
                   uint256 minimum, uint256 deadline, address recipient) private returns (Balances memory snapshot) {
        if (payer != address(0) || input == output || input == address(0) || output == address(0) ||
            recipient == address(0) || recipient == address(this) || block.timestamp > deadline ||
            budget == 0 || minimum == 0 || budget > uint256(type(int256).max) ||
            minimum > uint256(type(int256).max)) revert Order();
        payer = msg.sender;
        source = physicalIn;
        remaining = budget;
        snapshot.input = IERC20(input).balanceOf(msg.sender);
        snapshot.output = IERC20(output).balanceOf(recipient);
        snapshot.nativeAmount = address(this).balance;
        snapshot.usesNative = physicalIn == NATIVE || physicalOut == NATIVE;
        if (snapshot.usesNative) snapshot.wrappedAmount = wrapped.balanceOf(address(this));
    }

    function finish(Balances memory before, address input, address output, address physicalOut, address recipient,
                    uint256 expectedDebit, uint256 nominalOutput, uint256 minimum) private returns (uint256 spent, uint256 received) {
        if (physicalOut == NATIVE) {
            uint256 credit = address(this).balance - before.nativeAmount;
            if (credit != nominalOutput) revert Payment();
            wrapped.deposit{value: credit}();
            tokenCall(address(wrapped), abi.encodeCall(IERC20.transfer, (recipient, credit)));
        }
        spent = before.input - IERC20(input).balanceOf(msg.sender);
        received = IERC20(output).balanceOf(recipient) - before.output;
        if (paid != expectedDebit || spent != paid || received < minimum || address(this).balance != before.nativeAmount ||
            (before.usesNative && wrapped.balanceOf(address(this)) != before.wrappedAmount)) revert Payment();
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
