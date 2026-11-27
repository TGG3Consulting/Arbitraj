// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface IBancorVenue {
    function tradeBySourceAmount(address,address,uint256,uint256,uint256,address) external payable returns(uint256);
    function tradeByTargetAmount(address,address,uint256,uint256,uint256,address) external payable returns(uint256);
    function tradeBySourceAmountArb(address,address,uint256,uint256,uint256,address) external payable returns(uint256);
    function tradeByTargetAmountArb(address,address,uint256,uint256,uint256,address) external payable returns(uint256);
    function isWhitelisted(address) external view returns(bool);
}
interface IBancorRead {
    function network() external view returns(address);
    function masterVault() external view returns(address);
}
interface IBancorERC20 {
    function balanceOf(address) external view returns(uint256);
    function allowance(address,address) external view returns(uint256);
    function approve(address,uint256) external returns(bool);
    function transfer(address,uint256) external returns(bool);
    function transferFrom(address,address,uint256) external returns(bool);
    function deposit() external payable;
    function withdraw(uint256) external;
}

/// Uses Bancor's original exact-input or exact-output entry point. The cap, refund and net recipient
/// payout are enforced at the wallet even for non-standard ERC20s. No inventory or approval persists.
contract BancorV3Adapter {
    IBancorVenue public immutable network;
    IBancorRead public immutable info;
    address public immutable vault;
    address public immutable wrapped;
    address public immutable nativeToken;
    bool transient private entered;
    struct Order {
        address assetIn;
        address assetOut;
        uint256 form;
        uint256 maximum;
        uint256 minimum;
        uint256 deadline;
        bool exactOutput;
    }
    struct Initial {
        uint256 moduleNative;
        uint256 moduleSource;
        uint256 ownerSource;
        uint256 recipientTarget;
        uint256 vaultSource;
    }
    event Paid(address indexed payer,address indexed recipient,address indexed venue,address source,address target,
               uint256 form,bool exactOutput,uint256 spent,uint256 received);

    constructor(IBancorVenue n, IBancorRead i, address v, address w, address eth) {
        require(i.network()==address(n) && i.masterVault()==v && v.code.length!=0 && w.code.length!=0 &&
                address(n).code.length!=0 && eth!=address(0) && eth.code.length==0 && w!=eth,"roles");
        network=n;info=i;vault=v;wrapped=w;nativeToken=eth;
    }
    receive() external payable {}

    function execute(Order calldata order) external payable returns(uint256 spent,uint256 received) {
        return settle(order,msg.sender);
    }
    function swap(IBalancerVault.SingleSwap calldata s, IBalancerVault.FundManagement calldata f,
                  uint256 limit,uint256 deadline) external returns(uint256 received) {
        require(s.kind==0 && s.userData.length==0 && f.sender==msg.sender && !f.fromInternalBalance && !f.toInternalBalance
                && s.assetIn!=address(0) && s.assetOut!=address(0),"step");
        uint256 form=uint256(s.poolId);
        if (network.isWhitelisted(address(this))) form |= 4;
        (,received)=settle(Order(s.assetIn,s.assetOut,form,s.amount,limit,deadline,false),f.recipient);
    }
    function cash(address token,address owner) private view returns(uint256){
        return token==address(0)?owner.balance:IBancorERC20(token).balanceOf(owner);
    }
    function safe(address token,bytes memory data) private {
        (bool ok,bytes memory result)=token.call(data);
        require(ok && (result.length==0 || abi.decode(result,(bool))),"erc20");
    }
    function underlying(address asset,bool nativeSide) private view returns(address){
        if(nativeSide){
            require(asset==address(0) || asset==wrapped,"cash");
            return nativeToken;
        }
        require(asset!=nativeToken && asset!=address(0),"token");
        return asset;
    }
    function trade(Order memory o,address source,address target,address recipient) private returns(uint256){
        uint256 value=source==nativeToken?o.maximum:0;
        if(o.form & 4 != 0){
            require(network.isWhitelisted(address(this)),"exempt");
            return o.exactOutput?network.tradeByTargetAmountArb{value:value}(source,target,o.minimum,o.maximum,o.deadline,recipient):
                network.tradeBySourceAmountArb{value:value}(source,target,o.maximum,o.minimum,o.deadline,recipient);
        }
        return o.exactOutput?network.tradeByTargetAmount{value:value}(source,target,o.minimum,o.maximum,o.deadline,recipient):
            network.tradeBySourceAmount{value:value}(source,target,o.maximum,o.minimum,o.deadline,recipient);
    }
    function settle(Order memory o,address recipient) private returns(uint256 spent,uint256 received){
        require(!entered && o.form<8 && o.maximum>0 && o.minimum>0 && block.timestamp<=o.deadline &&
                recipient!=address(0) && recipient!=address(this) && recipient!=vault && msg.sender!=vault &&
                o.assetIn!=o.assetOut && msg.value==(o.assetIn==address(0)?o.maximum:0),"order");
        address source=underlying(o.assetIn,o.form & 1 != 0);
        address target=underlying(o.assetOut,o.form & 2 != 0);
        require(source!=target && info.network()==address(network) && info.masterVault()==vault,"market");
        entered=true;
        Initial memory old=Initial(address(this).balance-msg.value,cash(o.assetIn,address(this))-msg.value,
            cash(o.assetIn,msg.sender)+msg.value,cash(o.assetOut,recipient),cash(source==nativeToken?address(0):source,vault));
        if(o.assetIn!=address(0)){
            safe(o.assetIn,abi.encodeCall(IBancorERC20.transferFrom,(msg.sender,address(this),o.maximum)));
            require(cash(o.assetIn,address(this))==old.moduleSource+o.maximum,"partial deposit");
            if(source==nativeToken){IBancorERC20(wrapped).withdraw(o.maximum);}
            else{safe(source,abi.encodeCall(IBancorERC20.approve,(address(network),o.maximum)));}
        }
        bool wrapOut=target==nativeToken && o.assetOut==wrapped;
        uint256 result=trade(o,source,target,wrapOut?address(this):recipient);
        spent=o.exactOutput?result:o.maximum;
        require(spent>0 && spent<=o.maximum && cash(source==nativeToken?address(0):source,vault)-old.vaultSource==spent,"vault funding");
        if(source!=nativeToken && IBancorERC20(source).allowance(address(this),address(network))!=0){
            safe(source,abi.encodeCall(IBancorERC20.approve,(address(network),0)));
            require(IBancorERC20(source).allowance(address(this),address(network))==0,"allowance");
        }
        if(wrapOut){
            uint256 nativeOut=address(this).balance-old.moduleNative;
            IBancorERC20(wrapped).deposit{value:nativeOut}();
            safe(wrapped,abi.encodeCall(IBancorERC20.transfer,(recipient,nativeOut)));
        }
        uint256 refund=o.maximum-spent;
        if(refund>0){
            if(o.assetIn==address(0)){(bool ok,)=msg.sender.call{value:refund}("");require(ok,"native refund");}
            else{
                if(source==nativeToken){IBancorERC20(wrapped).deposit{value:refund}();}
                safe(o.assetIn,abi.encodeCall(IBancorERC20.transfer,(msg.sender,refund)));
            }
        }
        received=cash(o.assetOut,recipient)-old.recipientTarget;
        require(old.ownerSource-cash(o.assetIn,msg.sender)==spent && received>=o.minimum &&
                cash(o.assetIn,address(this))==old.moduleSource && address(this).balance==old.moduleNative,"wallet settlement");
        emit Paid(msg.sender,recipient,address(network),source,target,o.form,o.exactOutput,spent,received);
        entered=false;
    }
}
