// SPDX-License-Identifier: MIT
// Адаптер OasisSwap для вызова через swapFrom штатным ArbExecutor V5.
// Часть комиссии хранится на паре отдельно от резервов. Вход измеряется по текущему переводу,
// минимальный выход — по приросту баланса получателя. Развёртывание выполняет владелец.
pragma solidity 0.8.37;
interface IOasisRegistry {
    function getPair(address,address) external view returns(address);
    function getRebate(address) external view returns(uint64);
}
interface IOasisPair {
    function token0() external view returns(address);
    function token1() external view returns(address);
    function factory() external view returns(address);
    function fee() external view returns(uint64);
    function FEE_DIVISOR() external view returns(uint64);
    function calculateFee(uint64 rebate) external view returns(uint256);
    function getReserves() external view returns(uint112,uint112,uint32);
    function swap(uint256,uint256,address,bytes calldata) external;
    function swapCalculatingRebate(uint256,uint256,address,address,bytes calldata) external;
}
interface IOasisToken {
    function balanceOf(address) external view returns(uint256);
    function transferFrom(address,address,uint256) external returns(bool);
}

contract OasisSwapAdapter {
    IOasisRegistry public immutable factory;
    uint64 public constant MAX_REBATE=10_000;
    bool transient private entered;
    constructor(IOasisRegistry registry) {
        require(address(registry).code.length>0,"factory");
        factory=registry;
    }
    function swapFrom(address tokenIn,address tokenOut,uint256 amount,uint256 minimum,address recipient,uint256 deadline) external returns(uint256 received) {
        require(!entered && block.timestamp<=deadline && amount>0 && tokenIn!=tokenOut,"order");
        entered=true;
        IOasisPair pair=IOasisPair(factory.getPair(tokenIn,tokenOut));
        bool forward=tokenIn<tokenOut;
        require(address(pair)!=address(0) && pair.factory()==address(factory) &&
                pair.token0()==(forward?tokenIn:tokenOut) && pair.token1()==(forward?tokenOut:tokenIn),"market");
        require(recipient!=address(0) && recipient!=address(pair),"recipient");
        IOasisToken input=IOasisToken(tokenIn);
        IOasisToken output=IOasisToken(tokenOut);
        uint256 beforePool=input.balanceOf(address(pair));
        uint256 beforePayer=input.balanceOf(msg.sender);
        uint256 beforeOut=output.balanceOf(recipient);
        (bool ok,bytes memory data)=tokenIn.call(abi.encodeCall(input.transferFrom,(msg.sender,address(pair),amount)));
        require(ok && (data.length==0 || abi.decode(data,(bool))),"transfer");
        require(beforePayer-input.balanceOf(msg.sender)==amount,"spent");
        uint256 actual=input.balanceOf(address(pair))-beforePool;
        require(actual>0 && actual<=type(uint112).max,"input");
        // Протокол принимает tx.origin как получателя скидки. Право списания входных
        // токенов определяется отдельно: плательщик всегда msg.sender.
        uint64 rebate;
        try factory.getRebate(tx.origin) returns(uint64 value) { if (value<=MAX_REBATE) rebate=value; } catch {}
        (uint112 r0,uint112 r1,)=pair.getReserves();
        (uint256 rin,uint256 rout)=forward?(r0,r1):(r1,r0);
        uint256 den=pair.FEE_DIVISOR();
        uint256 rate=rebate>0?pair.calculateFee(rebate):pair.fee();
        require(den==10_000 && rate<den,"fee");
        uint256 adjusted=actual*(den-rate);
        uint256 out=adjusted*rout/(rin*den+adjusted);
        require(out>0,"empty");
        if(rebate>0) pair.swapCalculatingRebate(forward?0:out,forward?out:0,recipient,tx.origin,"");
        else pair.swap(forward?0:out,forward?out:0,recipient,"");
        received=output.balanceOf(recipient)-beforeOut;
        require(received>0 && received>=minimum,"payout");
        entered=false;
    }
}
