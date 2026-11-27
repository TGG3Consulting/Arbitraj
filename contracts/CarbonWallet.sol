// SPDX-License-Identifier: MIT
// Прямое исполнение Carbon средствами заявителя с проверкой суммы списания, обеспечения контроллера и чистого выхода.
// Модуль развёртывает владелец отдельно с адресом проверенного контроллера сети.
pragma solidity 0.8.37;

interface ICarbonWalletController {
    struct Action {uint256 strategyId; uint128 amount;}
    function calculateTradeSourceAmount(address source,address target,Action[] calldata actions) external view returns(uint128);
    function tradeBySourceAmount(address source,address target,Action[] calldata actions,uint256 deadline,uint128 minimum) external payable returns(uint128);
    function tradeByTargetAmount(address source,address target,Action[] calldata actions,uint256 deadline,uint128 maximum) external payable returns(uint128);
}
interface ICarbonWalletToken {
    function balanceOf(address owner) external view returns(uint256);
    function allowance(address owner,address spender) external view returns(uint256);
    function approve(address spender,uint256 amount) external returns(bool);
    function transfer(address to,uint256 amount) external returns(bool);
    function transferFrom(address from,address to,uint256 amount) external returns(bool);
}

contract CarbonWallet {
    address private constant NATIVE=0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;
    ICarbonWalletController public immutable controller;
    bool transient private entered;
    event Settled(address indexed payer,address indexed source,address indexed target,uint256 spent,uint256 received);
    constructor(ICarbonWalletController issuer){require(address(issuer).code.length>0,"controller");controller=issuer;}
    receive() external payable {require(entered && msg.sender==address(controller),"sender");}
    function balance(address token,address holder) private view returns(uint256){
        return token==NATIVE?holder.balance:ICarbonWalletToken(token).balanceOf(holder);
    }
    function tokenCall(address token,bytes memory data) private{
        (bool ok,bytes memory out)=token.call(data);
        require(ok && token.code.length!=0 && (out.length==0 || abi.decode(out,(bool))),"token");
    }
    function give(address token,uint256 n) private{
        if(n==0)return;
        if(token==NATIVE){(bool ok,)=msg.sender.call{value:n}("");require(ok,"native");}
        else tokenCall(token,abi.encodeCall(ICarbonWalletToken.transfer,(msg.sender,n)));
    }
    function execute(address source,address target,ICarbonWalletController.Action[] calldata actions,
        bool byTarget,uint256 deadline,uint128 limit) external payable returns(uint256 spent,uint256 received){
        require(!entered && source!=target && limit!=0 && actions.length>0 && block.timestamp<=deadline,"order");
        entered=true;
        uint128 fixedAmount;
        for(uint256 j;j<actions.length;++j){require(actions[j].amount!=0,"amount");fixedAmount+=actions[j].amount;}
        uint128 cap=byTarget?limit:fixedAmount;
        require(msg.value==(source==NATIVE?cap:0),"funding");
        uint256 localSource=balance(source,address(this))-msg.value;
        uint256 localTarget=balance(target,address(this));
        uint256 walletIn=balance(source,msg.sender)+msg.value;
        uint256 walletOut=balance(target,msg.sender);
        uint256 poolIn=balance(source,address(controller));
        uint256 poolOut=balance(target,address(controller));
        uint128 charge=byTarget?controller.calculateTradeSourceAmount(source,target,actions):fixedAmount;
        require(charge!=0 && charge<=cap,"cap");
        if(source!=NATIVE){
            tokenCall(source,abi.encodeCall(ICarbonWalletToken.transferFrom,(msg.sender,address(this),charge)));
            require(balance(source,address(this))==localSource+charge,"short funding");
            tokenCall(source,abi.encodeCall(ICarbonWalletToken.approve,(address(controller),charge)));
        }
        uint256 value=source==NATIVE?charge:0;
        uint128 nominal;
        if(byTarget){
            require(controller.tradeByTargetAmount{value:value}(source,target,actions,deadline,charge)==charge,"debit");
            nominal=fixedAmount;
        }else nominal=controller.tradeBySourceAmount{value:value}(source,target,actions,deadline,limit);
        require(balance(source,address(controller))-poolIn==charge && poolOut-balance(target,address(controller))==nominal,"controller balances");
        uint256 delivered=balance(target,address(this))-localTarget;
        require(delivered!=0 && delivered<=nominal,"payout");
        if(source==NATIVE)give(NATIVE,cap-charge);
        else if(ICarbonWalletToken(source).allowance(address(this),address(controller))!=0){
            tokenCall(source,abi.encodeCall(ICarbonWalletToken.approve,(address(controller),0)));
            require(ICarbonWalletToken(source).allowance(address(this),address(controller))==0,"allowance");
        }
        give(target,delivered);
        spent=walletIn-balance(source,msg.sender);
        received=balance(target,msg.sender)-walletOut;
        require(balance(source,address(this))==localSource && balance(target,address(this))==localTarget,"local balances");
        require(spent==charge && (byTarget?received==fixedAmount:received>=limit),"wallet");
        entered=false;
        emit Settled(msg.sender,source,target,spent,received);
    }
}
