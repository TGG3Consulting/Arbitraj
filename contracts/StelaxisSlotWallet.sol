// SPDX-License-Identifier: MIT
// Однократная заявка к исходной Ethereum-кассе Stelaxis в слоте опубликованной цены.
pragma solidity 0.8.37;

interface IStelaxis {
    function swapWithExtraFees(address input, address output, uint256 amount, uint256 minimum, uint256 deadline, uint256 extraFees) external;
}
interface IStelaxisToken {
    function transfer(address to, uint256 value) external returns (bool);
    function transferFrom(address from, address to, uint256 value) external returns (bool);
    function approve(address spender, uint256 value) external returns (bool);
    function balanceOf(address owner) external view returns (uint256);
    function allowance(address from,address to) external view returns (uint256);
}
contract StelaxisSlotWallet {
    address public immutable market;
    address public immutable registry;
    address public immutable vault;
    bytes32 public immutable marketHash;
    bytes32 public immutable registryHash;
    bytes32 public immutable vaultHash;

    struct Order {
        bytes32 leg; bytes32 parent; uint64 height; uint64 timestamp;
        address input; address output; uint256 amount; uint256 minimum;
    }
    mapping(address => mapping(bytes32 => bool)) public completed;
    bool transient private busy;
    error InvalidOrder();
    error Payment();
    event SlotFilled(bytes32 indexed leg,address indexed payer,address indexed venue,uint256 spent,uint256 received);

    constructor(address m,address r,address v,bytes32 mh,bytes32 rh,bytes32 vh) {
        if (block.chainid != 1 || m == r || m == v || r == v ||
            m.code.length == 0 || r.code.length == 0 || v.code.length == 0 ||
            m.codehash != mh || r.codehash != rh || v.codehash != vh) revert InvalidOrder();
        market=m; registry=r; vault=v; marketHash=mh; registryHash=rh; vaultHash=vh;
    }

    function slotVersion() external pure returns (uint256) { return 2; }

    function move(address token, bytes memory data) private {
        (bool ok,bytes memory result)=token.call(data);
        if (!ok || (result.length != 0 && !abi.decode(result,(bool)))) revert Payment();
    }

    function executeAtSlot(Order calldata o) external returns (uint256 spent,uint256 received) {
        if (busy || completed[msg.sender][o.leg] || o.leg == bytes32(0) || o.parent == bytes32(0) ||
            block.number != o.height || block.timestamp != o.timestamp || blockhash(block.number-1) != o.parent ||
            o.input == o.output || o.amount == 0 || o.minimum == 0 || msg.sender == vault || msg.sender == market ||
            market.codehash != marketHash || registry.codehash != registryHash || vault.codehash != vaultHash)
            revert InvalidOrder();
        busy=true;
        completed[msg.sender][o.leg]=true;
        IStelaxisToken input=IStelaxisToken(o.input);
        IStelaxisToken output=IStelaxisToken(o.output);
        uint256 ownerIn=input.balanceOf(msg.sender);
        uint256 ownerOut=output.balanceOf(msg.sender);
        uint256 moduleIn=input.balanceOf(address(this));
        uint256 moduleOut=output.balanceOf(address(this));
        uint256 vaultIn=input.balanceOf(vault);
        uint256 vaultOut=output.balanceOf(vault);
        if (input.allowance(address(this),market) != 0) revert Payment();
        move(o.input,abi.encodeCall(input.transferFrom,(msg.sender,address(this),o.amount)));
        if (input.balanceOf(address(this))-moduleIn != o.amount) revert Payment();
        move(o.input,abi.encodeCall(input.approve,(market,o.amount)));
        IStelaxis(market).swapWithExtraFees(o.input,o.output,o.amount,o.minimum,o.timestamp,0);
        if (input.allowance(address(this),market) != 0 || input.balanceOf(address(this)) != moduleIn ||
            input.balanceOf(vault)-vaultIn != o.amount) revert Payment();
        uint256 proceeds=output.balanceOf(address(this))-moduleOut;
        if (proceeds == 0 || vaultOut-output.balanceOf(vault) != proceeds) revert Payment();
        move(o.output,abi.encodeCall(output.transfer,(msg.sender,proceeds)));
        spent=ownerIn-input.balanceOf(msg.sender);
        received=output.balanceOf(msg.sender)-ownerOut;
        if (spent != o.amount || received < o.minimum || output.balanceOf(address(this)) != moduleOut) revert Payment();
        emit SlotFilled(o.leg,msg.sender,market,spent,received);
        busy=false;
    }
}
