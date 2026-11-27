// SPDX-License-Identifier: MIT
// Общий модуль одного кошелька для зарегистрированных пар SafeSwap на Ethereum.
// Комиссия ETH измеряется отдельно. При оплате из WETH аванс при продаже полностью
// возвращается из выручки, а конечный минимум проверяется по балансу получателя.
pragma solidity 0.8.37;

interface ISafeAsset {
    function balanceOf(address) external view returns (uint256);
    function allowance(address,address) external view returns (uint256);
    function approve(address,uint256) external returns (bool);
    function transfer(address,uint256) external returns (bool);
    function transferFrom(address,address,uint256) external returns (bool);
}
interface ISafeWrapped is ISafeAsset { function withdraw(uint256) external; }
interface ISafeFactory { function getPair(address,address) external view returns(address); }
interface ISafePair {function factory() external view returns(address); function token0() external view returns(address); function token1() external view returns(address);}
interface ISafeNativeRouter {
    function factory() external view returns(address);
    function WETH() external view returns(address);
    function getAmountsOut(uint256,address[] calldata) external view returns(uint256[] memory);
    function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[] calldata,address,uint256) external;
}
interface ISafeTrade {
    struct Trade { uint256 amountIn; uint256 amountOut; address[] path; address to; uint256 deadline; }
    function swapRouter() external view returns(address);
    function feePercent() external view returns(uint256);
    function percent() external view returns(uint256);
    function getFees(address[] calldata,uint256,address) external view returns(uint256,uint256,uint256,uint256);
    function swapExactTokensForTokensWithFeeAmount(Trade calldata) external payable;
    function swapExactETHForTokensWithFeeAmount(Trade calldata,uint256 feeAmount) external payable;
}

contract SafeSwapAdapter {
    address public immutable payer;
    address public immutable executor;
    ISafeNativeRouter public immutable router;
    ISafeTrade public immutable trade;
    ISafeFactory public immutable factory;
    ISafeWrapped public immutable wrapped;
    bool transient private entered;

    event Filled(address indexed sender,address indexed recipient,address indexed input,address output,
                 uint256 spent,uint256 received,uint256 nativeFee,bool separateFee);
    constructor(address wallet,address execution,ISafeFactory venue,ISafeNativeRouter spender,ISafeTrade entry,ISafeWrapped weth) {
        require(block.chainid==1 && wallet!=address(0) && wallet!=execution && address(weth).code.length!=0,"network");
        require(spender.factory()==address(venue) && spender.WETH()==address(weth),"router");
        require(address(entry)==address(0) || entry.swapRouter()==address(spender),"public");
        payer=wallet;executor=execution;factory=venue;router=spender;trade=entry;wrapped=weth;
    }

    receive() external payable {require(msg.sender==address(wrapped),"sender");}
    // Temporarily advance the native fee on V5 sales. The swap reinstates this ETH in the same transaction.
    uint256 public feeReserve;
    function fundFees() external payable {require(!entered && msg.sender==payer,"owner"); feeReserve+=msg.value;}
    function withdrawFees(uint256 amount) external {
        require(!entered && msg.sender==payer,"owner");
        entered=true;feeReserve-=amount;
        (bool ok,)=payable(payer).call{value:amount}("");require(ok,"payment");entered=false;
    }

    function _fee(address[] memory path,uint256 amount) private view returns(uint256 charged) {
        if(address(trade)==address(0)) return 0;
        (uint256 total,uint256 dex,uint256 tokenIn,uint256 tokenOut)=trade.getFees(path,amount,address(this));
        require(total==dex && tokenIn==0 && tokenOut==0,"fee terms");
        return dex;
    }
    function _assets(address input,address output) private view returns(address[] memory path) {
        require(input!=output && input.code.length!=0 && output.code.length!=0,"assets");
        address pool=factory.getPair(input,output);
        require(pool.code.length!=0 && ISafePair(pool).factory()==address(factory) &&
            (input<output ? (ISafePair(pool).token0()==input && ISafePair(pool).token1()==output) :
                            (ISafePair(pool).token0()==output && ISafePair(pool).token1()==input)),"registry");
        require(router.factory()==address(factory) && (address(trade)==address(0) ||
                trade.swapRouter()==address(router)),"roles");
        path=new address[](2);path[0]=input;path[1]=output;
    }
    function _terms(address[] memory path,uint256 amount,bool separate) private view returns(uint256 principal,uint256 fee) {
        fee=_fee(path,amount);
        principal=amount;
        bool buy=path[0]==address(wrapped);
        if(!separate && fee!=0) {
            if(buy) {
                // fee(n)=floor(n*rate/den). Some isolated one-wei budgets have no exact split.
                // An EOA may still send the separately quoted fee for those orders.
                uint256 rate=trade.feePercent();
                uint256 den=trade.percent();
                principal=amount-amount*rate/(den+rate);
                fee=_fee(path,principal);
                if(principal+fee>amount) {--principal;fee=_fee(path,principal);}
                require(principal!=0 && principal+fee==amount,"split");
            } else require(path[1]==address(wrapped) && feeReserve>=fee,"fee advance");
        }
    }

    // Amounts for the indicated module account. The included transaction still
    // checks the actual terminal minimum and can decline a halted or restricted pair.
    function quote(address input,address output,uint256 amount,bool separate) external view returns(uint256 net,uint256 fee) {
        address[] memory path=_assets(input,output);
        (uint256 principal,uint256 charged)=_terms(path,amount,separate);
        uint256[] memory gross=router.getAmountsOut(principal,path);
        require(gross.length==2 && gross[0]==principal && (address(trade)==address(0) || address(trade).balance==0 || input==address(wrapped)),"quote");
        net=gross[1];
        if(!separate && input!=address(wrapped)) net-=charged;
        return (net,charged);
    }
    function _move(ISafeAsset token,bytes memory data) private {
        (bool ok,bytes memory result)=address(token).call(data);
        require(ok && (result.length==0 || abi.decode(result,(bool))),"asset");
    }
    struct Order { address input;address output;uint256 amount;uint256 minimum;address recipient;uint256 deadline; }
    function execute(Order calldata order) external payable returns(uint256) {
        require(msg.sender==payer && order.recipient==payer && order.minimum!=0,"payer");
        return _settle(order,true);
    }
    function swapFrom(address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        external returns(uint256) {
        require(msg.sender==payer || msg.sender==executor,"payer");
        return _settle(Order(input,output,amount,minimum,recipient,deadline),false);
    }
    struct Opening {uint256 etherBase; uint256 src; uint256 dst; uint256 payerIn; uint256 recipientOut; uint256 publicETH;}
    function _swap(Order memory o,address[] memory path,uint256 principal,uint256 fee,
                   address destination,bool nativeInput,bool repay) private {
        if(address(trade)==address(0)) {
            router.swapExactTokensForTokensSupportingFeeOnTransferTokens(principal,o.minimum,path,destination,o.deadline);
        } else if(nativeInput) {
            wrapped.withdraw(principal);
            trade.swapExactETHForTokensWithFeeAmount{value:principal+fee}(
                ISafeTrade.Trade(principal,o.minimum,path,destination,o.deadline),fee);
        } else {
            trade.swapExactTokensForTokensWithFeeAmount{value:fee}(ISafeTrade.Trade(principal,o.minimum+(repay?fee:0),path,destination,o.deadline));
        }
    }
    function _settle(Order memory o,bool separate) private returns(uint256 credit) {
        require(!entered && o.amount!=0 && block.timestamp<=o.deadline && o.recipient!=address(0) &&
                o.recipient!=address(this) && o.recipient!=address(router) && o.recipient!=address(trade),"order");
        entered=true;
        address[] memory path=_assets(o.input,o.output);
        require(o.recipient!=factory.getPair(o.input,o.output),"recipient");
        (uint256 principal,uint256 fee)=_terms(path,o.amount,separate);
        bool buy=o.input==address(wrapped);
        require(msg.value==(separate?fee:0),"fee value");
        Opening memory initial;
        initial.publicETH=address(trade)==address(0)?0:address(trade).balance;
        // Only native exact-input explicitly pays feeJar(dexFee) and forwards precisely
        // principal to the internal router. Token-input selectors forward the entire ETH balance.
        bool nativeInput=initial.publicETH!=0;
        require(!nativeInput || buy,"fee balance");
        ISafeAsset src=ISafeAsset(o.input);ISafeAsset dst=ISafeAsset(o.output);
        initial.etherBase=address(this).balance-msg.value;
        initial.src=src.balanceOf(address(this));initial.dst=dst.balanceOf(address(this));
        initial.payerIn=src.balanceOf(msg.sender);initial.recipientOut=dst.balanceOf(o.recipient);
        require(src.allowance(address(this),address(router))==0,"previous allowance");
        _move(src,abi.encodeCall(src.transferFrom,(msg.sender,address(this),o.amount)));
        require(src.balanceOf(address(this))-initial.src==o.amount,"funding");
        if(!separate && buy && fee!=0) wrapped.withdraw(fee);
        if(!nativeInput) _move(src,abi.encodeCall(src.approve,(address(router),principal)));
        bool repay=!separate && !buy && fee!=0;
        address destination=repay?address(this):o.recipient;
        _swap(o,path,principal,fee,destination,nativeInput,repay);
        if(src.allowance(address(this),address(router))!=0) _move(src,abi.encodeCall(src.approve,(address(router),0)));
        if(repay) {
            uint256 gross=dst.balanceOf(address(this))-initial.dst;
            require(gross>fee,"reimbursement");
            wrapped.withdraw(fee);
            _move(dst,abi.encodeCall(dst.transfer,(o.recipient,gross-fee)));
        }
        credit=dst.balanceOf(o.recipient)-initial.recipientOut;
        require(credit!=0 && credit>=o.minimum && initial.payerIn-src.balanceOf(msg.sender)==o.amount,"net");
        require(src.balanceOf(address(this))==initial.src && dst.balanceOf(address(this))==initial.dst &&
                address(this).balance==initial.etherBase && (address(trade)==address(0) || address(trade).balance==initial.publicETH) && src.allowance(address(this),address(router))==0,"residual");
        emit Filled(msg.sender,o.recipient,o.input,o.output,o.amount,credit,fee,separate);
        entered=false;
    }
}
