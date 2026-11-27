// SPDX-License-Identifier: MIT
// Одна зарегистрированная пара DEGENSwap Ethereum. Плательщик задаёт номинальный вход,
// роутер получает только фактически зачисленные модулю токены, конечный минимум — по кошельку.
pragma solidity 0.8.37;

import {ISafeAsset, ISafeWrapped, ISafeFactory, ISafePair, ISafeNativeRouter} from "./SafeSwapAdapter.sol";

contract DegenSwapAdapter {
    address public immutable payer;
    address public immutable executor;
    ISafeFactory public immutable factory;
    ISafeNativeRouter public immutable router;
    address public immutable trade;
    ISafeWrapped public immutable wrapped;
    bool transient private entered;
    event Filled(address indexed sender,address indexed recipient,address indexed input,address output,
                 uint256 spent,uint256 received,uint256 nativeFee,bool separateFee);
    event Funded(address indexed sender,address indexed input,uint256 spent,uint256 credited);
    constructor(address wallet,address execution,ISafeFactory venue,ISafeNativeRouter spender,address entry,ISafeWrapped weth){
        require(block.chainid==1 && wallet!=address(0) && wallet!=execution && entry==address(0),"roles");
        require(address(weth).code.length>0 && spender.factory()==address(venue) && spender.WETH()==address(weth),"router");
        payer=wallet;executor=execution;factory=venue;router=spender;trade=entry;wrapped=weth;
    }
    function feeReserve() external pure returns(uint256){return 0;}
    function move(ISafeAsset token,bytes memory callData) private {
        (bool ok,bytes memory result)=address(token).call(callData);
        require(ok && (result.length==0 || abi.decode(result,(bool))),"transfer");
    }
    struct Opening {uint256 walletIn;uint256 receiverOut;uint256 moduleIn;uint256 moduleOut;}
    function swapFrom(address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline) external returns(uint256 net){
        require(!entered && (msg.sender==payer || msg.sender==executor) && recipient!=address(0) && recipient!=address(this) && recipient!=address(router),"payer");
        require(block.timestamp<=deadline && minimum!=0 && amount!=0 && input!=output && input.code.length>0 && output.code.length>0,"order");
        entered=true;
        address pair=factory.getPair(input,output);
        require(pair.code.length>0 && recipient!=pair && ISafePair(pair).factory()==address(factory),"pair");
        (address a,address b)=input<output?(input,output):(output,input);
        require(ISafePair(pair).token0()==a && ISafePair(pair).token1()==b && router.factory()==address(factory),"assets");
        ISafeAsset src=ISafeAsset(input);ISafeAsset dst=ISafeAsset(output);
        Opening memory start=Opening(src.balanceOf(msg.sender),dst.balanceOf(recipient),src.balanceOf(address(this)),dst.balanceOf(address(this)));
        require(src.allowance(address(this),address(router))==0,"initial allowance");
        move(src,abi.encodeCall(src.transferFrom,(msg.sender,address(this),amount)));
        uint256 credited=src.balanceOf(address(this))-start.moduleIn;
        require(credited>0 && credited<=amount,"funding");
        move(src,abi.encodeCall(src.approve,(address(router),credited)));
        address[] memory path=new address[](2);path[0]=input;path[1]=output;
        router.swapExactTokensForTokensSupportingFeeOnTransferTokens(credited,minimum,path,recipient,deadline);
        move(src,abi.encodeCall(src.approve,(address(router),0)));
        uint256 spent=start.walletIn-src.balanceOf(msg.sender);
        net=dst.balanceOf(recipient)-start.receiverOut;
        require(spent==amount && net>=minimum && net!=0,"net");
        require(src.balanceOf(address(this))==start.moduleIn && dst.balanceOf(address(this))==start.moduleOut
                && src.allowance(address(this),address(router))==0,"residual");
        emit Funded(msg.sender,input,spent,credited);
        emit Filled(msg.sender,recipient,input,output,spent,net,0,false);
        entered=false;
    }
}
