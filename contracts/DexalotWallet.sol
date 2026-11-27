// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// https://github.com/Dexalot/contracts/blob/993df50c014f3c8a1a33e45b4364ab9e709662fb/contracts/MainnetRFQ.sol
// The venue applies its time-dependent slippage table *after* checking the maker's signature.
interface IDxSettlement {
    struct Order {
        uint256 nonceAndMeta;
        uint128 expiry;
        address makerAsset;
        address takerAsset;
        address maker;
        address taker;
        uint256 makerAmount;
        uint256 takerAmount;
    }
    function getAllowedRFQs() external view returns(address[] memory);
    function simpleSwap(Order calldata order,bytes calldata signature) external payable;
    function partialSwap(Order calldata order,bytes calldata signature,uint256 amount) external payable;
}
interface IDxAsset {
    function approve(address spender,uint256 amount) external returns(bool);
    function transfer(address to,uint256 amount) external returns(bool);
    function transferFrom(address from,address to,uint256 amount) external returns(bool);
    function balanceOf(address holder) external view returns(uint256);
    function allowance(address holder,address spender) external view returns(uint256);
}
interface IDxWrapped is IDxAsset {
    function deposit() external payable;
    function withdraw(uint256 amount) external;
}

contract DexalotWallet {
    // MainnetRFQ._verifyOrder извлекает адрес получателя из старших 160 битов подписанного nonceAndMeta.
    // https://github.com/Dexalot/contracts/blob/993df50c014f3c8a1a33e45b4364ab9e709662fb/contracts/MainnetRFQ.sol#L680-L701
    uint256 private constant DEST_TRADER_SHIFT = 96;

    address public owner;
    IDxSettlement public immutable router;
    IDxWrapped public immutable wrapped;
    bytes32 public immutable routerHash;
    address transient private activeMaker;
    error InvalidOrder();
    error Payment();
    event Filled(address indexed payer,address indexed venue,uint256 indexed nonce,
                 address input,address output,uint256 spent,uint256 received);

    constructor(IDxSettlement market,IDxWrapped weth) {
        if(address(market).code.length==0 || address(weth).code.length==0) revert InvalidOrder();
        owner=msg.sender;
        router=market;
        wrapped=weth;
        routerHash=address(market).codehash;
    }

    receive() external payable {
        if(activeMaker==address(0) || (msg.sender!=activeMaker && msg.sender!=address(wrapped) && msg.sender!=address(router)))
            revert Payment();
    }

    function execute(IDxSettlement.Order calldata order,bytes calldata signature,uint256 maximum,uint256 minimum,
                     bool partialFill,uint256 deadline) external returns(uint256 spent,uint256 received) {
        address payee=address(uint160(order.nonceAndMeta >> DEST_TRADER_SHIFT));
        bool nativeIn=order.takerAsset==address(0);
        bool nativeOut=order.makerAsset==address(0);
        address input=nativeIn?address(wrapped):order.takerAsset;
        address output=nativeOut?address(wrapped):order.makerAsset;
        // The bot accounts in ERC20. A native payout must arrive here so it can be wrapped in the same transaction.
        // Подпись покрывает весь uint128 expiry. Для допуска MainnetRFQ сравнивает время с младшими
        // 32 битами включительно: https://github.com/Dexalot/contracts/blob/993df50c014f3c8a1a33e45b4364ab9e709662fb/contracts/MainnetRFQ.sol#L696-L704
        // https://github.com/Dexalot/contracts/blob/993df50c014f3c8a1a33e45b4364ab9e709662fb/contracts/MainnetRFQ.sol#L752-L762
        if(msg.sender!=owner || activeMaker!=address(0) || order.maker==address(0) ||
           address(router).codehash!=routerHash || input==output || signature.length==0 ||
           (order.taker!=owner && order.taker!=address(this)) || (payee!=owner && payee!=address(this)) ||
           (nativeOut && payee!=address(this)) || !isMaker(order.maker) || block.timestamp>deadline ||
           block.timestamp>uint32(order.expiry) || maximum==0 || minimum==0 || order.takerAmount==0 ||
           order.makerAmount==0 || (!partialFill && order.takerAmount>maximum)) revert InvalidOrder();
        // Some generations accept a partial amount above the signed amount without increasing their payout.
        uint256 amount=maximum<order.takerAmount?maximum:order.takerAmount;
        activeMaker=order.maker;
        IDxAsset src=IDxAsset(input);
        IDxAsset dst=IDxAsset(output);
        uint256 beforeIn=src.balanceOf(owner);
        uint256 beforeOut=dst.balanceOf(owner);
        uint256 heldIn=src.balanceOf(address(this));
        uint256 heldOut=dst.balanceOf(address(this));
        uint256 heldNative=address(this).balance;
        if(src.allowance(address(this),address(router))!=0) revert Payment();
        checked(src,abi.encodeCall(src.transferFrom,(owner,address(this),amount)));
        uint256 funded=src.balanceOf(address(this))-heldIn;
        if(funded==0 || funded>amount || (!partialFill && funded!=amount)) revert Payment();
        if(nativeIn) wrapped.withdraw(funded);
        else checked(src,abi.encodeCall(src.approve,(address(router),funded)));
        if(partialFill) router.partialSwap{value:nativeIn?funded:0}(order,signature,funded);
        else router.simpleSwap{value:nativeIn?funded:0}(order,signature);
        if(src.allowance(address(this),address(router))!=0)
            checked(src,abi.encodeCall(src.approve,(address(router),0)));
        uint256 newEther=address(this).balance-heldNative;
        if(newEther!=0){
            if(!nativeIn && !nativeOut) revert Payment();
            wrapped.deposit{value:newEther}();
        }
        uint256 refund=src.balanceOf(address(this))-heldIn;
        if(refund>funded) revert Payment();
        if(refund!=0) checked(src,abi.encodeCall(src.transfer,(owner,refund)));
        uint256 payout=dst.balanceOf(address(this))-heldOut;
        if(payee==address(this) && payout!=0) checked(dst,abi.encodeCall(dst.transfer,(owner,payout)));
        spent=beforeIn-src.balanceOf(owner);
        received=dst.balanceOf(owner)-beforeOut;
        if(spent==0 || spent>maximum || received<minimum || src.balanceOf(address(this))!=heldIn ||
           dst.balanceOf(address(this))!=heldOut || address(this).balance!=heldNative ||
           src.allowance(address(this),address(router))!=0) revert Payment();
        emit Filled(owner,order.maker,order.nonceAndMeta,input,output,spent,received);
        activeMaker=address(0);
    }

    function isMaker(address maker) private view returns(bool) {
        address[] memory allowed=router.getAllowedRFQs();
        for(uint256 i;i<allowed.length;++i) if(allowed[i]==maker) return true;
        return false;
    }

    function checked(IDxAsset token,bytes memory data) private {
        (bool ok,bytes memory result)=address(token).call(data);
        if(!ok || (result.length!=0 && !abi.decode(result,(bool)))) revert Payment();
    }
}
