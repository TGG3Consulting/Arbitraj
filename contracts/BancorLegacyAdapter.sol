// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface ILegacyAnchor {
    function owner() external view returns(address);
}
interface ILegacyRegistry {
    function addressOf(bytes32 name) external view returns (address);
}
interface ILegacyPools {
    function registry() external view returns (ILegacyRegistry);
    function isLiquidityPool(address anchor) external view returns (bool);
}
interface ILegacyNetwork {
    function convertByPath(address[] memory path, uint256 amount, uint256 minimum,
                           address beneficiary, address affiliate, uint256 fee) external payable returns (uint256);
}
interface ILegacyAsset {
    function balanceOf(address wallet) external view returns (uint256);
    function allowance(address wallet,address spender) external view returns (uint256);
    function approve(address spender,uint256 amount) external returns (bool);
    function transfer(address recipient,uint256 amount) external returns (bool);
    function transferFrom(address payer,address recipient,uint256 amount) external returns (bool);
    function deposit() external payable;
    function withdraw(uint256 amount) external;
}

/// Bancor v2.1 uses anchor addresses between reserves. Its router takes a minimum but no deadline.
contract BancorLegacyAdapter {
    ILegacyPools public immutable pools;
    ILegacyNetwork public immutable network;
    address public immutable wrapped;
    address private constant NATIVE = 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;
    bool transient private entered;

    struct Order {
        address source;
        address target;
        uint256 amount;
        uint256 minimum;
        uint256 deadline;
        address[] path;
    }
    struct Cash {
        uint256 nativeBalance;
        uint256 moduleSource;
        uint256 payerSource;
        uint256 recipientTarget;
        uint256 reserveSource;
    }
    event Paid(address indexed payer,address indexed recipient,bytes32 indexed pathHash,address source,address target,uint256 spent,uint256 received);

    constructor(ILegacyPools registry,ILegacyNetwork router,address weth) {
        require(address(router).code.length>0 && weth.code.length>0, "roles");
        pools=registry;
        network=router;
        wrapped=weth;
        require(rolesMatch(), "registry");
    }
    receive() external payable {}

    function rolesMatch() private view returns (bool) {
        ILegacyRegistry registry = pools.registry();
        return registry.addressOf("BancorNetwork")==address(network) &&
               registry.addressOf("BancorConverterRegistry")==address(pools);
    }
    function tokenCall(address token,bytes memory data) private {
        (bool ok, bytes memory result)=token.call(data);
        require(ok && token.code.length!=0 && (result.length==0 || abi.decode(result,(bool))), "token");
    }
    function balance(address token,address wallet) private view returns (uint256) {
        return token==address(0) ? wallet.balance : ILegacyAsset(token).balanceOf(wallet);
    }
    function endpoint(address token,address reserve) private view returns(bool) {
        return reserve==NATIVE ? (token==wrapped || token==address(0)) : (token!=address(0) && token==reserve);
    }
    function execute(Order calldata order) external payable returns(uint256 received) {
        return settle(order,msg.sender);
    }
    function swap(IBalancerVault.SingleSwap calldata leg,IBalancerVault.FundManagement calldata funds,
                  uint256 minimum,uint256 deadline) external returns(uint256 received) {
        uint256 key=uint256(leg.poolId);
        require(leg.kind==0 && leg.userData.length==0 && (key>>162)==0 && funds.sender==msg.sender &&
                !funds.fromInternalBalance && !funds.toInternalBalance && leg.assetIn!=address(0) && leg.assetOut!=address(0),"step");
        address[] memory path = new address[](3);
        path[0]=((key>>160)&1)==0 ? leg.assetIn : NATIVE;
        path[1]=address(uint160(key));
        path[2]=((key>>161)&1)==0 ? leg.assetOut : NATIVE;
        return settle(Order(leg.assetIn,leg.assetOut,leg.amount,minimum,deadline,path),funds.recipient);
    }
    function settle(Order memory o,address recipient) private returns(uint256 received) {
        require(!entered && block.timestamp<=o.deadline && o.amount!=0 && o.minimum!=0 &&
                o.source!=o.target && recipient!=address(0) && recipient!=address(this),"order");
        entered=true;
        uint256 last=o.path.length;
        require(last>=3 && (last&1)==1 && endpoint(o.source,o.path[0]) && endpoint(o.target,o.path[last-1]) &&
                rolesMatch(),"path");
        for(uint256 i=1;i<last;i+=2) require(pools.isLiquidityPool(o.path[i]),"anchor");
        bool nativeIn=o.path[0]==NATIVE;
        bool wrapOut=o.path[last-1]==NATIVE && o.target==wrapped;
        require(msg.value==(o.source==address(0) ? o.amount : 0),"value");
        address first=ILegacyAnchor(o.path[1]).owner();
        address reserve=nativeIn ? address(0) : o.source;
        Cash memory before=Cash(address(this).balance-msg.value,balance(o.source,address(this))-msg.value,
                                balance(o.source,msg.sender)+msg.value,balance(o.target,recipient),balance(reserve,first));
        if(o.source!=address(0)){
            tokenCall(o.source,abi.encodeCall(ILegacyAsset.transferFrom,(msg.sender,address(this),o.amount)));
            require(balance(o.source,address(this))-before.moduleSource==o.amount,"funding");
            if(nativeIn) ILegacyAsset(wrapped).withdraw(o.amount);
            else tokenCall(o.source,abi.encodeCall(ILegacyAsset.approve,(address(network),o.amount)));
        }
        uint256 reported=network.convertByPath{value:nativeIn ? o.amount : 0}(
            o.path,o.amount,o.minimum,wrapOut ? address(this) : recipient,address(0),0);
        require(balance(reserve,first)-before.reserveSource==o.amount,"reserve credit");
        if(wrapOut){
            uint256 credit=address(this).balance-before.nativeBalance;
            require(credit==reported,"native credit");
            ILegacyAsset(wrapped).deposit{value:credit}();
            tokenCall(wrapped,abi.encodeCall(ILegacyAsset.transfer,(recipient,credit)));
        }
        if(!nativeIn && ILegacyAsset(o.source).allowance(address(this),address(network))!=0){
            tokenCall(o.source,abi.encodeCall(ILegacyAsset.approve,(address(network),0)));
            require(ILegacyAsset(o.source).allowance(address(this),address(network))==0,"allowance");
        }
        require(address(this).balance==before.nativeBalance && balance(o.source,address(this))==before.moduleSource &&
                before.payerSource-balance(o.source,msg.sender)==o.amount,"debit");
        received=balance(o.target,recipient)-before.recipientTarget;
        require(received>=o.minimum,"net credit");
        emit Paid(msg.sender,recipient,keccak256(abi.encode(o.path)),o.source,o.target,o.amount,received);
        entered=false;
    }
}
