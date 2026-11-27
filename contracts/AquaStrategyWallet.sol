// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Модуль размещает владелец. Расход и чистую выплату он сверяет по физическим
// остаткам. Прямой вызов содержит всю программу Aqua. Для V5 kind 17 ключ
// указывает на однажды сохранённую неизменяемую программу и порядок переводов.
// https://github.com/1inch/swap-vm/tree/v1.0.2
interface IAquaEngine {
    struct Order { address maker; uint256 traits; bytes data; }
    function AQUA() external view returns (address);
    function quote(Order calldata order,address tokenIn,address tokenOut,uint256 amount,bytes calldata taker)
        external view returns(uint256,uint256,bytes32);
    function swap(Order calldata order,address tokenIn,address tokenOut,uint256 amount,bytes calldata taker)
        external returns(uint256,uint256,bytes32);
}
interface IAquaAsset {
    function balanceOf(address) external view returns(uint256);
    function allowance(address,address) external view returns(uint256);
    function approve(address,uint256) external returns(bool);
    function transferFrom(address,address,uint256) external returns(bool);
}

contract AquaStrategyWallet {
    // SwapVM v1.0.2, src/libs/MakerTraits.sol#L30 — ордер обеспечен остатками Aqua.
    // https://github.com/1inch/swap-vm/blob/v1.0.2/src/libs/MakerTraits.sol#L30
    uint256 private constant USE_AQUA_INSTEAD_OF_SIGNATURE = 1 << 254;

    // https://github.com/1inch/swap-vm/blob/v1.0.2/src/libs/TakerTraits.sol#L93-L99
    uint16 private constant EXACT_IN = 0x0001;
    uint16 private constant FIRST_TRANSFER_FROM_TAKER = 0x0020;
    uint16 private constant TRANSFER_FROM_AND_AQUA_PUSH = 0x0040;

    // Поля threshold, to и deadline следуют подряд; остальные поля пусты.
    // https://github.com/1inch/swap-vm/blob/v1.0.2/src/libs/TakerTraits.sol#L105-L136
    uint160 private constant THRESHOLD_END = 32;
    uint160 private constant RECIPIENT_END = THRESHOLD_END + 20;
    uint160 private constant DEADLINE_END = RECIPIENT_END + 5;
    uint256 private constant OFFSET_BITS = 16;
    uint256 private constant DEADLINE_INDEX = 2;
    uint256 private constant OFFSET_COUNT = 10;

    struct Terms { address input; address output; uint256 amount; uint256 minimum; uint256 deadline; bool inputFirst; }
    struct SingleSwap { bytes32 poolId; uint8 kind; address assetIn; address assetOut; uint256 amount; bytes userData; }
    struct Funds { address sender; bool fromInternal; address recipient; bool toInternal; }
    IAquaEngine public immutable engine;
    address public immutable registry;
    bytes32 public immutable engineCode;
    bytes32 public immutable v5Code;
    mapping(bytes32 => bytes) private programs;
    mapping(bytes32 => bytes32) public orderForKey;
    mapping(bytes32 => bool) private inputFirstForKey;
    bool private busy;
    error Invalid();
    event Cached(bytes32 indexed orderHash);
    event Settled(bytes32 indexed orderHash,address indexed payer,address indexed recipient,
        address input,address output,uint256 paid,uint256 received,bool inputFirst);

    constructor(IAquaEngine swapVM,address aqua,bytes32 executorHash) {
        if(address(swapVM).code.length==0 || aqua.code.length==0 || swapVM.AQUA()!=aqua || executorHash==0) revert Invalid();
        engine=swapVM; registry=aqua; engineCode=address(swapVM).codehash; v5Code=executorHash;
    }

    function cache(IAquaEngine.Order calldata order) external returns(bytes32 h) {
        h=orderHash(order);
        if(programs[h].length!=0) return h;
        programs[h]=abi.encode(order);
        orderForKey[key(h,false)]=h;
        bytes32 second=key(h,true);
        orderForKey[second]=h; inputFirstForKey[second]=true;
        emit Cached(h);
    }

    function key(bytes32 h,bool inputFirst) public pure returns(bytes32) {
        return keccak256(abi.encode(h,inputFirst));
    }
    function orderHash(IAquaEngine.Order memory order) public pure returns(bytes32) {
        if(order.maker==address(0) || order.traits & USE_AQUA_INSTEAD_OF_SIGNATURE==0) revert Invalid();
        return keccak256(abi.encode(order));
    }

    function _traits(Terms memory t,address recipient) private pure returns(bytes memory) {
        uint160 stops = THRESHOLD_END | (RECIPIENT_END << OFFSET_BITS);
        for (uint256 i = DEADLINE_INDEX; i < OFFSET_COUNT; ++i) {
            stops |= DEADLINE_END << (OFFSET_BITS * i);
        }
        uint16 flags = EXACT_IN | TRANSFER_FROM_AND_AQUA_PUSH;
        if (t.inputFirst) flags |= FIRST_TRANSFER_FROM_TAKER;
        return abi.encodePacked(stops,flags,t.minimum,recipient,uint40(t.deadline));
    }
    // В вызове от кошелька SwapVM видит тот же tx.origin и того же taker, что при оплате модулем.
    function preview(IAquaEngine.Order calldata order,Terms calldata t,address recipient) external view returns(uint256 net) {
        if(recipient==address(0) || t.deadline<block.timestamp || t.deadline>type(uint40).max
            || address(engine).codehash!=engineCode || engine.AQUA()!=registry) revert Invalid();
        (uint256 paid,uint256 out,bytes32 h)=engine.quote(order,t.input,t.output,t.amount,_traits(t,recipient));
        if(paid!=t.amount || h!=orderHash(order)) revert Invalid();
        return out;
    }
    function execute(IAquaEngine.Order calldata order,Terms calldata terms) external returns(uint256) {
        return _settle(order,terms,msg.sender);
    }
    function swap(SingleSwap calldata s,Funds calldata f,uint256 limit,uint256 deadline) external returns(uint256) {
        if(msg.sender.codehash!=v5Code || f.sender!=msg.sender || f.fromInternal || f.toInternal
            || s.kind!=0 || s.userData.length!=0) revert Invalid();
        bytes32 h=orderForKey[s.poolId];
        if(h==bytes32(0)) revert Invalid();
        IAquaEngine.Order memory order=abi.decode(programs[h],(IAquaEngine.Order));
        return _settle(order,Terms(s.assetIn,s.assetOut,s.amount,limit,deadline,inputFirstForKey[s.poolId]),f.recipient);
    }

    function _settle(IAquaEngine.Order memory order,Terms memory t,address recipient) private returns(uint256 got) {
        if(busy || recipient==address(0) || recipient==address(this) || msg.sender==address(this)
            || t.amount==0 || t.minimum==0 || t.deadline<block.timestamp || t.deadline>type(uint40).max
            || t.input==t.output || t.input.code.length==0 || t.output.code.length==0
            || address(engine).codehash!=engineCode || engine.AQUA()!=registry) revert Invalid();
        busy=true;
        bytes32 h=orderHash(order);
        IAquaAsset src=IAquaAsset(t.input);
        IAquaAsset dst=IAquaAsset(t.output);
        uint256 startIn=src.balanceOf(address(this));
        uint256 startOut=dst.balanceOf(address(this));
        uint256 ownerIn=src.balanceOf(msg.sender);
        uint256 ownerOut=dst.balanceOf(recipient);
        checked(t.input,abi.encodeCall(IAquaAsset.transferFrom,(msg.sender,address(this),t.amount)));
        if(src.balanceOf(address(this))!=startIn+t.amount || src.allowance(address(this),address(engine))!=0) revert Invalid();
        checked(t.input,abi.encodeCall(IAquaAsset.approve,(address(engine),t.amount)));
        bytes memory taker=_traits(t,recipient);
        (uint256 paid,,bytes32 actual)=engine.swap(order,t.input,t.output,t.amount,taker);
        if(src.allowance(address(this),address(engine))!=0) checked(t.input,abi.encodeCall(IAquaAsset.approve,(address(engine),0)));
        got=dst.balanceOf(recipient)-ownerOut;
        if(actual!=h || paid!=t.amount || ownerIn-src.balanceOf(msg.sender)!=t.amount || got<t.minimum
            || src.allowance(address(this),address(engine))!=0 || src.balanceOf(address(this))!=startIn
            || dst.balanceOf(address(this))!=startOut) revert Invalid();
        emit Settled(h,msg.sender,recipient,t.input,t.output,paid,got,t.inputFirst);
        busy=false;
    }
    function checked(address asset,bytes memory data) private {
        (bool ok,bytes memory ret)=asset.call(data);
        if(!ok || (ret.length!=0 && !abi.decode(ret,(bool)))) revert Invalid();
    }
}
