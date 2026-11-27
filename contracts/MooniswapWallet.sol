// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Файл: MooniswapWallet.sol. Кто вызывает: кошелёк плательщика или шаг 17 ArbExecutor.
// Что делает: ограничивает срок обмена и проверяет чистое получение выбранной физической валюты.
// Зависит от реестра конкретного поколения 1inch Mooniswap, его пула и контракта WETH.
import {IBalancerVault} from "./IPools.sol";
interface IMoonFactory {
    function isPool(address pool) external view returns(bool);
    function pools(address a,address b) external view returns(address);
}
interface IMoonPool {
    function swap(address src,address dst,uint256 amount,uint256 minReturn,address referral) external payable returns(uint256);
    function swapFor(address src,address dst,uint256 amount,uint256 minReturn,address referral,address payable receiver) external payable returns(uint256);
}
interface IMoonAsset {
    function balanceOf(address a) external view returns(uint256);
    function allowance(address a,address spender) external view returns(uint256);
    function approve(address a,uint256 amount) external returns(bool);
    function transfer(address a,uint256 amount) external returns(bool);
    function transferFrom(address from,address a,uint256 amount) external returns(bool);
    function deposit() external payable;
    function withdraw(uint256) external;
}
contract MooniswapWallet {
    IMoonFactory public immutable factory;
    IMoonAsset public immutable wrapped;
    bool public immutable hasSwapFor;
    bool transient private busy;
    struct Order {bytes32 market;address input;address output;uint256 amount;uint256 minimum;uint256 deadline;}
    struct Assets {address pool;address src;address dst;}
    struct Balances {uint256 eth;uint256 weth;uint256 moduleIn;uint256 moduleOut;uint256 payerIn;uint256 recipientOut;}
    event Paid(address indexed payer,address indexed recipient,address indexed pool,address input,address output,
               uint256 spent,uint256 received);
    error Market();
    error Bounds();
    error Funds();

    constructor(IMoonFactory registry,IMoonAsset wrapper,bool supportsReceiver) {
        if(address(registry).code.length==0||address(wrapper).code.length==0||address(registry)==address(wrapper)) revert Market();
        factory=registry;wrapped=wrapper;hasSwapFor=supportsReceiver;
    }
    receive() external payable {}

    function execute(Order calldata order) external payable returns(uint256) {
        return settle(order,payable(msg.sender));
    }
    function swap(IBalancerVault.SingleSwap calldata route,IBalancerVault.FundManagement calldata funds,
                  uint256 minimum,uint256 deadline) external returns(uint256) {
        if(route.kind!=0||route.userData.length!=0||funds.sender!=msg.sender||funds.fromInternalBalance||
           funds.toInternalBalance||route.assetIn==address(0)||route.assetOut==address(0)) revert Market();
        return settle(Order(route.poolId,route.assetIn,route.assetOut,route.amount,minimum,deadline),funds.recipient);
    }
    function physical(address asset,bool nativeSide) private view returns(address) {
        if(nativeSide) {
            if(asset!=address(0)&&asset!=address(wrapped)) revert Market();
            return address(0);
        }
        return asset==address(0)?address(wrapped):asset;
    }
    function assets(Order memory o) private view returns(Assets memory a) {
        uint256 flags=uint256(o.market)>>160;
        if(flags>2||o.input==o.output) revert Market();
        a.pool=address(uint160(uint256(o.market)));
        a.src=physical(o.input,flags==1);
        a.dst=physical(o.output,flags==2);
        if(a.src==a.dst||!factory.isPool(a.pool)||factory.pools(a.src,a.dst)!=a.pool||
           factory.pools(a.dst,a.src)!=a.pool) revert Market();
    }
    function cash(address token,address owner) private view returns(uint256) {
        return token==address(0)?owner.balance:IMoonAsset(token).balanceOf(owner);
    }
    function callToken(address token,bytes memory data) private {
        (bool ok,bytes memory out)=token.call(data);
        if(!ok||(out.length!=0&&!abi.decode(out,(bool)))) revert Funds();
    }
    function pay(address token,address payable recipient,uint256 amount) private {
        if(token==address(0)) {
            (bool ok,)=recipient.call{value:amount}("");
            if(!ok) revert Funds();
        } else { callToken(token,abi.encodeCall(IMoonAsset.transfer,(recipient,amount))); }
    }
    function convert(address from,address to,uint256 amount) private {
        if(from==to) return;
        if(from==address(0)&&to==address(wrapped)) wrapped.deposit{value:amount}();
        else if(from==address(wrapped)&&to==address(0)) wrapped.withdraw(amount);
        else revert Market();
    }
    function settle(Order memory o,address payable recipient) private returns(uint256 received) {
        if(busy||o.amount==0||o.minimum==0||block.timestamp>o.deadline||
           msg.value!=(o.input==address(0)?o.amount:0)) revert Bounds();
        busy=true;
        Assets memory a=assets(o);
        if(recipient==address(0)||recipient==address(this)||recipient==a.pool||msg.sender==a.pool) revert Market();
        Balances memory b=Balances(address(this).balance-msg.value,wrapped.balanceOf(address(this)),
            cash(o.input,address(this))-msg.value,cash(o.output,address(this)),cash(o.input,msg.sender)+msg.value,
            cash(o.output,recipient));
        uint256 use=o.amount;
        if(o.input!=address(0)){
            callToken(o.input,abi.encodeCall(IMoonAsset.transferFrom,(msg.sender,address(this),o.amount)));
            use=cash(o.input,address(this))-b.moduleIn;
            if(use==0||use>o.amount) revert Funds();
        }
        convert(o.input,a.src,use);
        if(a.src!=address(0)){
            if(IMoonAsset(a.src).allowance(address(this),a.pool)!=0) revert Funds();
            callToken(a.src,abi.encodeCall(IMoonAsset.approve,(a.pool,use)));
        }
        bool direct=hasSwapFor&&a.dst==o.output;
        uint256 beforePay=cash(a.dst,address(this));
        uint256 value=a.src==address(0)?use:0;
        if(hasSwapFor) IMoonPool(a.pool).swapFor{value:value}(a.src,a.dst,use,o.minimum,address(0),direct?recipient:payable(address(this)));
        else IMoonPool(a.pool).swap{value:value}(a.src,a.dst,use,o.minimum,address(0));
        if(a.src!=address(0)&&IMoonAsset(a.src).allowance(address(this),a.pool)!=0) {
            callToken(a.src,abi.encodeCall(IMoonAsset.approve,(a.pool,0)));
            if(IMoonAsset(a.src).allowance(address(this),a.pool)!=0) revert Funds();
        }
        if(!direct){
            uint256 gain=cash(a.dst,address(this))-beforePay;
            convert(a.dst,o.output,gain);
            pay(o.output,recipient,gain);
        }
        received=cash(o.output,recipient)-b.recipientOut;
        if(received<o.minimum||b.payerIn-cash(o.input,msg.sender)!=o.amount||
           address(this).balance!=b.eth||wrapped.balanceOf(address(this))!=b.weth||
           cash(o.input,address(this))!=b.moduleIn||cash(o.output,address(this))!=b.moduleOut) revert Funds();
        emit Paid(msg.sender,recipient,a.pool,o.input,o.output,o.amount,received);
        busy=false;
    }
}
