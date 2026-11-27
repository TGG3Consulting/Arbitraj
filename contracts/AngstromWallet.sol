// SPDX-License-Identifier: MIT
// Владелец размещает этот модуль для проверенного AngstromAdapter, хука и PoolManager одной сети.
// Кошелёк задаёт физический PoolKey, блок аттестации, полный расход и чистый минимум выплаты.
// При частичном расходе или недостаточной выплате вся операция откатывается.
pragma solidity 0.8.37;

interface IAngstromSettlement {
    struct Key { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
    struct Attestation { uint64 blockNumber; bytes unlockData; }
    function poolManager() external view returns (address);
    function swap(Key calldata key, bool zeroForOne, uint128 input, uint128 minimum,
        Attestation[] calldata bundle, address recipient, uint256 deadline) external returns (uint256);
}

interface IAngstromAsset {
    function balanceOf(address account) external view returns (uint256);
    function allowance(address account, address spender) external view returns (uint256);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function approve(address spender, uint256 amount) external returns (bool);
}

contract AngstromWallet {
    IAngstromSettlement public immutable adapter;
    address public immutable hook;
    address public immutable manager;
    bool transient private active;
    error InvalidMarket();
    error InvalidPayment();
    event Paid(bytes32 indexed poolId, address indexed payer, address indexed input,
        address output, uint128 spent, uint256 received, uint64 height);

    constructor(address venue, address angstrom, address poolManager) {
        if (venue.code.length == 0 || angstrom.code.length == 0 || poolManager.code.length == 0 ||
            IAngstromSettlement(venue).poolManager() != poolManager) revert InvalidMarket();
        adapter = IAngstromSettlement(venue);
        hook = angstrom;
        manager = poolManager;
    }

    function execute(IAngstromSettlement.Key calldata key, bool zeroForOne, uint128 amount,
        uint128 minimum, uint64 height, bytes calldata attestation, uint256 deadline) external returns (uint256 received) {
        if (key.hooks != hook || key.currency0 == address(0) || key.currency0 >= key.currency1 ||
            key.fee != 0x800000 || key.tickSpacing <= 0) revert InvalidMarket();
        if (active || amount == 0 || minimum == 0 || height != block.number || block.timestamp > deadline)
            revert InvalidPayment();
        active = true;
        IAngstromAsset input = IAngstromAsset(zeroForOne ? key.currency0 : key.currency1);
        IAngstromAsset output = IAngstromAsset(zeroForOne ? key.currency1 : key.currency0);
        uint256 initial = input.balanceOf(address(this));
        uint256 ownerIn = input.balanceOf(msg.sender);
        uint256 ownerOut = output.balanceOf(msg.sender);
        tokenCall(address(input), abi.encodeCall(input.transferFrom,(msg.sender,address(this),amount)));
        if (input.balanceOf(address(this)) != initial + amount ||
            input.allowance(address(this),address(adapter)) != 0) revert InvalidPayment();
        tokenCall(address(input),abi.encodeCall(input.approve,(address(adapter),amount)));
        IAngstromSettlement.Attestation[] memory bundle = new IAngstromSettlement.Attestation[](1);
        bundle[0] = IAngstromSettlement.Attestation(height,attestation);
        adapter.swap(key,zeroForOne,amount,minimum,bundle,msg.sender,deadline);
        if (input.allowance(address(this),address(adapter)) != 0) {
            tokenCall(address(input),abi.encodeCall(input.approve,(address(adapter),0)));
        }
        if (input.allowance(address(this),address(adapter)) != 0 || input.balanceOf(address(this)) != initial ||
            ownerIn - input.balanceOf(msg.sender) != amount) revert InvalidPayment();
        received = output.balanceOf(msg.sender) - ownerOut;
        if (received < minimum) revert InvalidPayment();
        emit Paid(keccak256(abi.encode(key)),msg.sender,address(input),address(output),amount,received,height);
        active = false;
    }

    function tokenCall(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        if (!ok || (result.length != 0 && !abi.decode(result,(bool)))) revert InvalidPayment();
    }
}
