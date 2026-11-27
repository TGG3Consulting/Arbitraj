// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

import {IBalancerVault} from "./IPools.sol";

interface IAmbientDex {
    function userCmd(uint16 callpath,bytes calldata command) external payable returns(bytes memory);
    function readSlot(uint256 slot) external view returns(uint256);
}
interface IAmbientQuery {
    function dex_() external view returns(address);
    function queryPrice(address base,address quote,uint256 poolIdx) external view returns(uint128);
}
interface IAmbientAsset {
    function balanceOf(address holder) external view returns(uint256);
    function allowance(address holder,address spender) external view returns(uint256);
    function approve(address spender,uint256 value) external returns(bool);
    function transfer(address target,uint256 value) external returns(bool);
    function transferFrom(address payer,address target,uint256 value) external returns(bool);
}
interface IAmbientWrapped is IAmbientAsset {
    function withdraw(uint256 amount) external;
    function deposit() external payable;
}

// https://docs.ambient.finance/developers/dex-contract-interface/swaps/hot-path-swap-migration/directly-calling-crocswapdex
// Ethereum HotProxy checks only the variable leg. Check the recipient's net proceeds independently.
contract AmbientWallet {
    struct Market { address base; address quote; uint256 index; }
    IAmbientDex public immutable dex;
    IAmbientQuery public immutable query;
    IAmbientWrapped public immutable wrapped;
    bytes32 public immutable venueHash;
    bytes32 public immutable hotHash;
    bytes32 public immutable knockoutHash;
    address public immutable hot;
    address public immutable knockout;
    mapping(bytes32=>Market) private registered;
    bool transient private entered;

    error InvalidMarket();
    error InvalidOrder();
    error Payment();
    event Filled(bytes32 indexed market,address indexed payer,address indexed recipient,
                 address input,address output,uint256 spent,uint256 received);

    constructor(IAmbientDex venue,IAmbientQuery lens,IAmbientWrapped weth) {
        if(address(venue).code.length==0 || lens.dex_()!=address(venue) || address(weth).code.length==0)
            revert InvalidMarket();
        dex=venue;
        query=lens;
        wrapped=weth;
        venueHash=address(venue).codehash;
        // proxyPaths_[1] and proxyPaths_[3500] follow the packed flags at slot 0.
        hot=address(uint160(venue.readSlot(2)));
        knockout=address(uint160(venue.readSlot(3501)));
        if(hot.code.length==0 || knockout.code.length==0) revert InvalidMarket();
        hotHash=hot.codehash;
        knockoutHash=knockout.codehash;
    }

    receive() external payable {
        if(msg.sender!=address(wrapped) && msg.sender!=address(dex)) revert Payment();
    }
    function key(Market memory m) public pure returns(bytes32) {
        return keccak256(abi.encode(m.base,m.quote,m.index));
    }
    function register(Market calldata m) external {
        if(m.base>=m.quote || query.queryPrice(m.base,m.quote,m.index)==0) revert InvalidMarket();
        registered[key(m)]=m;
    }
    function canonical(address a) private view returns(address) {
        return a==address(0)?address(wrapped):a;
    }
    function common(address a,address b) private pure returns(Market memory) {
        return a<b ? Market(a,b,420) : Market(b,a,420);
    }
    function resolve(bytes32 id,address src,address dst) public view returns(Market memory m) {
        m=common(src,dst);
        if(key(m)==id) return m;
        if(src==address(wrapped) || dst==address(wrapped)){
            m=common(address(0),src==address(wrapped)?dst:src);
            if(key(m)==id) return m;
        }
        m=registered[id];
        if(m.base>=m.quote || key(m)!=id) revert InvalidMarket();
    }

    function execute(Market calldata m,address input,address output,uint128 budget,uint128 minimum,
                     bool exactOut,uint128 limitPrice,uint256 deadline) external returns(uint256,uint256) {
        return settle(m,input,output,budget,minimum,exactOut,limitPrice,deadline,msg.sender,false);
    }

    function swap(IBalancerVault.SingleSwap calldata o,IBalancerVault.FundManagement calldata f,
                  uint256 minimum,uint256 deadline) external returns(uint256 received) {
        if(o.kind!=0 || o.userData.length!=0 || f.sender!=msg.sender || f.fromInternalBalance ||
           f.toInternalBalance || o.amount>uint128(type(int128).max) || minimum>uint128(type(int128).max))
            revert InvalidOrder();
        Market memory m=resolve(o.poolId,o.assetIn,o.assetOut);
        (,received)=settle(m,o.assetIn,o.assetOut,uint128(o.amount),uint128(minimum),false,0,deadline,f.recipient,true);
    }

    function settle(Market memory m,address input,address output,uint128 budget,uint128 minimum,bool exactOut,
                    uint128 limit,uint256 deadline,address recipient,bool fullSpend) private returns(uint256 spent,uint256 received) {
        if(entered || input==output || input==address(0) || output==address(0) || recipient==address(0) ||
           recipient==address(this) || budget==0 || minimum==0 || budget>uint128(type(int128).max) ||
           minimum>uint128(type(int128).max) || block.timestamp>deadline) revert InvalidOrder();
        entered=true;
        bool buy=input==canonical(m.base);
        if(m.base>=m.quote || input!=canonical(buy?m.base:m.quote) || output!=canonical(buy?m.quote:m.base) ||
           address(dex).codehash!=venueHash || dex.readSlot(2)!=uint160(hot) || dex.readSlot(3501)!=uint160(knockout) ||
           hot.codehash!=hotHash || knockout.codehash!=knockoutHash) revert InvalidMarket();
        bool fromNative=(buy?m.base:m.quote)==address(0);
        bool toNative=(buy?m.quote:m.base)==address(0);
        IAmbientAsset src=IAmbientAsset(input);
        IAmbientAsset dst=IAmbientAsset(output);
        uint256 payer=src.balanceOf(msg.sender);
        uint256 payee=dst.balanceOf(recipient);
        uint256 inventoryIn=src.balanceOf(address(this));
        uint256 inventoryOut=dst.balanceOf(address(this));
        uint256 etherBefore=address(this).balance;
        tokenCall(src,abi.encodeCall(src.transferFrom,(msg.sender,address(this),budget)));
        uint256 funded=src.balanceOf(address(this))-inventoryIn;
        if(funded==0 || funded>budget || src.allowance(address(this),address(dex))!=0) revert Payment();
        if(fromNative) wrapped.withdraw(funded);
        else tokenCall(src,abi.encodeCall(src.approve,(address(dex),funded)));
        // TickMath bounds from the deployed Ambient curve. A caller-specified limit may restrict a fill.
        if(limit==0) limit=buy ? 21267430153580247136652501917186561137 : 65538;
        bytes memory cmd=abi.encode(m.base,m.quote,m.index,buy,exactOut?!buy:buy,
                                    exactOut?minimum:uint128(funded),uint16(0),limit,
                                    exactOut?uint128(funded):minimum,uint8(0));
        bytes memory result=dex.userCmd{value:fromNative?funded:0}(1,cmd);
        (int128 baseFlow,int128 quoteFlow)=abi.decode(result,(int128,int128));
        int256 debit=buy?int256(baseFlow):int256(quoteFlow);
        int256 credit=buy?-int256(quoteFlow):-int256(baseFlow);
        if(debit<=0 || credit<=0 || uint256(debit)>funded) revert Payment();
        if(src.allowance(address(this),address(dex))!=0) tokenCall(src,abi.encodeCall(src.approve,(address(dex),0)));
        uint256 etherDelta=address(this).balance-etherBefore;
        if(fromNative || toNative){
            if(etherDelta!=(fromNative?funded-uint256(debit):uint256(credit))) revert Payment();
            if(etherDelta!=0) wrapped.deposit{value:etherDelta}();
        }
        uint256 refund=src.balanceOf(address(this))-inventoryIn;
        if(refund!=funded-uint256(debit)) revert Payment();
        if(refund!=0) tokenCall(src,abi.encodeCall(src.transfer,(msg.sender,refund)));
        uint256 payout=dst.balanceOf(address(this))-inventoryOut;
        if(payout==0 || payout>uint256(credit)) revert Payment();
        tokenCall(dst,abi.encodeCall(dst.transfer,(recipient,payout)));
        spent=payer-src.balanceOf(msg.sender);
        received=dst.balanceOf(recipient)-payee;
        if(spent==0 || spent>budget || (fullSpend && spent!=budget) || received<minimum ||
           address(this).balance!=etherBefore || src.balanceOf(address(this))!=inventoryIn ||
           dst.balanceOf(address(this))!=inventoryOut || src.allowance(address(this),address(dex))!=0) revert Payment();
        emit Filled(key(m),msg.sender,recipient,input,output,spent,received);
        entered=false;
    }
    function tokenCall(IAmbientAsset token,bytes memory data) private {
        (bool ok,bytes memory ret)=address(token).call(data);
        if(!ok || (ret.length!=0 && !abi.decode(ret,(bool)))) revert Payment();
    }
}
