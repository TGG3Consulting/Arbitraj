// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Оплата ордера Raindex из котировочной валюты через отдельную пару Router02.
// Модуль измеряет три ERC20 и возвращает плательщику сдачу по исходной валюте.
interface IRainCashToken {
    function balanceOf(address owner) external view returns (uint256);
    function allowance(address owner,address spender) external view returns (uint256);
    function approve(address spender,uint256 value) external returns (bool);
    function transfer(address to,uint256 value) external returns (bool);
    function transferFrom(address from,address to,uint256 value) external returns (bool);
}
interface IRainCashBook {
    struct Limits {address input;address output;uint256 minimumDebit;uint256 maximumDebit;uint256 minimumCredit;uint256 deadline;}
    function fill(Limits calldata limits,bytes calldata action) external returns(uint256 spent,uint256 received);
}
interface IRainCashRouter {
    function factory() external view returns(address);
    function swapTokensForExactTokens(uint256 out,uint256 cap,address[] calldata path,address to,uint256 deadline) external returns(uint256[] memory);
    function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256 input,uint256 floor,address[] calldata path,address to,uint256 deadline) external;
}
interface IRainCashFactory {function getPair(address,address) external view returns(address);}

contract RaindexCashWallet {
    error InvalidRoute();
    error Payment();
    address public immutable bookWallet;
    address public immutable router;
    address public immutable pair;
    address public immutable cash;
    address public immutable bridge;
    bytes32 public immutable walletCode;
    bytes32 public immutable routerCode;
    bytes32 public immutable pairCode;
    bool private transient entered;

    constructor(address wallet_,address router_,address pair_,address cash_,address bridge_) {
        if(wallet_.code.length==0||router_.code.length==0||pair_.code.length==0||
           cash_.code.length==0||bridge_.code.length==0||cash_==bridge_||
           IRainCashFactory(IRainCashRouter(router_).factory()).getPair(cash_,bridge_)!=pair_) revert InvalidRoute();
        bookWallet=wallet_;router=router_;pair=pair_;cash=cash_;bridge=bridge_;
        walletCode=wallet_.codehash;routerCode=router_.codehash;pairCode=pair_.codehash;
    }

    struct Request {address asset;uint256 cap;uint256 floor;uint256 bridgeLimit;uint256 deadline;bool buy;}
    event Settled(address indexed owner,bytes32 indexed action,address indexed asset,bool buy,uint256 spent,uint256 received,uint256 intermediate);

    function fill(Request calldata r, bytes calldata action) external returns(uint256 spent,uint256 received){
        if(entered||r.cap==0||r.floor==0||r.bridgeLimit==0||r.asset==cash||r.asset==bridge||
           r.asset.code.length==0||block.timestamp>r.deadline) revert Payment();
        if(bookWallet.codehash!=walletCode||router.codehash!=routerCode||pair.codehash!=pairCode||
           IRainCashFactory(IRainCashRouter(router).factory()).getPair(cash,bridge)!=pair) revert InvalidRoute();
        entered=true;
        address input=r.buy?cash:r.asset;
        address output=r.buy?r.asset:cash;
        uint256 initial=balance(input,address(this));
        uint256 originalOut=balance(output,address(this));
        uint256 originalBridge=balance(bridge,address(this));
        uint256 ownerIn=balance(input,msg.sender);
        uint256 ownerOut=balance(output,msg.sender);
        transfer(input,abi.encodeCall(IRainCashToken.transferFrom,(msg.sender,address(this),r.cap)));
        if(balance(input,address(this))!=initial+r.cap) revert Payment();
        uint256 middle=r.buy?purchase(r,action,originalBridge):sale(r,action,originalBridge);
        if(balance(bridge,address(this))!=originalBridge) revert Payment();
        uint256 change=balance(input,address(this))-initial;
        spent=r.cap-change;
        if(spent==0||(!r.buy && spent!=r.cap)) revert Payment();
        if(change!=0) transfer(input,abi.encodeCall(IRainCashToken.transfer,(msg.sender,change)));
        uint256 credit=balance(output,address(this))-originalOut;
        if(credit<r.floor) revert Payment();
        transfer(output,abi.encodeCall(IRainCashToken.transfer,(msg.sender,credit)));
        received=balance(output,msg.sender)-ownerOut;
        if(ownerIn-balance(input,msg.sender)!=spent||received<r.floor||balance(input,address(this))!=initial||balance(output,address(this))!=originalOut) revert Payment();
        emit Settled(msg.sender,keccak256(action),r.asset,r.buy,spent,received,middle);
        entered=false;
    }

    function purchase(Request calldata r,bytes calldata action,uint256 beforeBridge) private returns(uint256) {
        address[] memory path=new address[](2);path[0]=cash;path[1]=bridge;
        approve(cash,router,r.cap);
        IRainCashRouter(router).swapTokensForExactTokens(r.bridgeLimit,r.cap,path,address(this),r.deadline);
        revoke(cash,router);
        if(balance(bridge,address(this))-beforeBridge!=r.bridgeLimit) revert Payment();
        approve(bridge,bookWallet,r.bridgeLimit);
        (uint256 paid,) = IRainCashBook(bookWallet).fill(IRainCashBook.Limits(bridge,r.asset,1,r.bridgeLimit,r.floor,r.deadline),action);
        revoke(bridge,bookWallet);
        uint256 leftover=balance(bridge,address(this))-beforeBridge;
        if(paid==0 || paid+leftover!=r.bridgeLimit) revert Payment();
        if(leftover!=0){
            path[0]=bridge;path[1]=cash;
            approve(bridge,router,leftover);
            IRainCashRouter(router).swapExactTokensForTokensSupportingFeeOnTransferTokens(leftover,1,path,address(this),r.deadline);
            revoke(bridge,router);
        }
        return paid;
    }

    function sale(Request calldata r,bytes calldata action,uint256 beforeBridge) private returns(uint256 middle) {
        approve(r.asset,bookWallet,r.cap);
        IRainCashBook(bookWallet).fill(IRainCashBook.Limits(r.asset,bridge,r.cap,r.cap,r.bridgeLimit,r.deadline),action);
        revoke(r.asset,bookWallet);
        middle=balance(bridge,address(this))-beforeBridge;
        if(middle<r.bridgeLimit) revert Payment();
        address[] memory path=new address[](2);path[0]=bridge;path[1]=cash;
        approve(bridge,router,middle);
        IRainCashRouter(router).swapExactTokensForTokensSupportingFeeOnTransferTokens(middle,r.floor,path,address(this),r.deadline);
        revoke(bridge,router);
    }

    function balance(address token,address who) private view returns(uint256){return IRainCashToken(token).balanceOf(who);}
    function approve(address token,address spender,uint256 amount) private {
        if(IRainCashToken(token).allowance(address(this),spender)!=0) revert Payment();
        transfer(token,abi.encodeCall(IRainCashToken.approve,(spender,amount)));
    }
    function revoke(address token,address spender) private {
        if(IRainCashToken(token).allowance(address(this),spender)!=0) transfer(token,abi.encodeCall(IRainCashToken.approve,(spender,0)));
        if(IRainCashToken(token).allowance(address(this),spender)!=0) revert Payment();
    }
    function transfer(address token,bytes memory payload) private{
        (bool ok,bytes memory ret)=token.call(payload);
        if(!ok||(ret.length!=0&&!abi.decode(ret,(bool)))) revert Payment();
    }
}
