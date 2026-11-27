// SPDX-License-Identifier: MIT
// Владелец размещает модуль с проверенными роутером и фабриками выбранной сети. Модуль обменивает средства
// плательщика между PT и разрешённым ERC-20 данной серии. Подписанный срок и минимум действуют до чистой выплаты.
pragma solidity 0.8.37;
import {IBalancerVault} from "./IPools.sol";

interface IPendleCurrency {
    function balanceOf(address who) external view returns(uint256);
    function allowance(address who,address spender) external view returns(uint256);
    function approve(address spender,uint256 amount) external returns(bool);
    function transferFrom(address from,address to,uint256 amount) external returns(bool);
}
interface IPendleMarketInfo {
    function factory() external view returns(address);
    function readTokens() external view returns(address sy,address pt,address yt);
    function expiry() external view returns(uint256);
}
interface IPendleRegistrar { function isValidMarket(address market) external view returns(bool); }
interface IPendleSeries {
    function SY() external view returns(address);
    function YT() external view returns(address);
    function PT() external view returns(address);
    function expiry() external view returns(uint256);
}
interface IPendleSy {
    function isValidTokenIn(address token) external view returns(bool);
    function isValidTokenOut(address token) external view returns(bool);
}
interface IPendleWalletRouter {
    struct SwapData {uint8 swapType; address extRouter; bytes extCalldata; bool needScale;}
    struct Terminal {address token;uint256 amount;address syToken;address pendleSwap;SwapData swapData;}
    struct Order {uint256 salt;uint256 expiry;uint256 nonce;uint8 orderType;address token;address yt;address maker;address receiver;uint256 making;uint256 lnRate;uint256 failSafe;bytes permit;}
    struct Fill {Order order;bytes signature;uint256 amount;}
    struct Limit {address router;uint256 eps;Fill[] normal;Fill[] flash;bytes data;}
    function swapExactSyForPtSimple(address receiver,address market,uint256 exactSy,uint256 minimum) external returns(uint256,uint256);
    function swapExactPtForSy(address receiver,address market,uint256 exactPt,uint256 minimum,Limit calldata orders) external returns(uint256,uint256);
    function swapExactTokenForPtSimple(address receiver,address market,uint256 minimum,Terminal calldata input) external payable returns(uint256,uint256,uint256);
    function swapExactPtForToken(address receiver,address market,uint256 exactPt,Terminal calldata output,Limit calldata orders) external returns(uint256,uint256,uint256);
}
contract PendleMarketAdapter {
    IPendleWalletRouter public immutable router;
    address public immutable originalFactory;
    address public immutable factoryV3;
    address public immutable factoryV4;
    address public immutable factoryV5;
    address public immutable factoryV6;
    bool private transient busy;
    error InvalidMarket();
    error InvalidOrder();
    error Payment();
    event Settled(address indexed payer,address indexed recipient,address indexed market,address input,address output,uint256 spent,uint256 received);
    constructor(uint256 expectedChain,address router_,address[5] memory registered) {
        if (block.chainid!=expectedChain || router_.code.length==0) revert InvalidMarket();
        router=IPendleWalletRouter(router_);
        bool hasFactory;
        for(uint256 i;i<registered.length;++i) {
            if(registered[i]==address(0))continue;
            hasFactory=true;
            if(registered[i].code.length==0)revert InvalidMarket();
            for(uint256 j;j<i;++j)if(registered[i]==registered[j])revert InvalidMarket();
        }
        if(!hasFactory)revert InvalidMarket();
        originalFactory=registered[0];factoryV3=registered[1];factoryV4=registered[2];factoryV5=registered[3];factoryV6=registered[4];
    }
    function execute(address market,address input,address output,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256) {
        return _trade(market,input,output,amount,minimum,msg.sender,deadline);
    }
    // V5 шаг 17 передаёт адрес конкретного рынка в младших 160 битах poolId; адаптер сам проверяет его фабрику и PT/SY/YT.
    function swap(IBalancerVault.SingleSwap calldata o,IBalancerVault.FundManagement calldata f,uint256 minimum,uint256 deadline)
        external payable returns(uint256) {
        if(msg.value!=0 || o.kind!=0 || o.userData.length!=0 || f.sender!=msg.sender || f.fromInternalBalance ||
            f.toInternalBalance || uint256(o.poolId)>type(uint160).max) revert InvalidOrder();
        return _trade(address(uint160(uint256(o.poolId))),o.assetIn,o.assetOut,o.amount,minimum,f.recipient,deadline);
    }
    function _trade(address market,address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        private returns(uint256 received) {
        if(busy || amount==0 || minimum==0 || block.timestamp>deadline || input==output || input==address(0) || output==address(0)
            || recipient==address(0) || recipient==address(this) || recipient==market || recipient==address(router)) revert InvalidOrder();
        busy=true;
        IPendleMarketInfo venue=IPendleMarketInfo(market);address registrar=venue.factory();
        if(registrar==address(0) || !(registrar==originalFactory || registrar==factoryV3 || registrar==factoryV4 || registrar==factoryV5 || registrar==factoryV6)
            || !IPendleRegistrar(registrar).isValidMarket(market) || venue.expiry()<=block.timestamp) revert InvalidMarket();
        (address sy,address pt,address yt)=venue.readTokens();bool buy=output==pt;
        if(IPendleSeries(pt).SY()!=sy || IPendleSeries(pt).YT()!=yt || IPendleSeries(yt).SY()!=sy || IPendleSeries(yt).PT()!=pt
            || IPendleSeries(pt).expiry()!=venue.expiry() || IPendleSeries(yt).expiry()!=venue.expiry() || recipient==sy || recipient==yt
            || recipient==pt || (buy? input!=sy && !IPendleSy(sy).isValidTokenIn(input)
                                     : input!=pt || (output!=sy && !IPendleSy(sy).isValidTokenOut(output)))
            || input==yt || output==yt) revert InvalidMarket();
        IPendleCurrency src=IPendleCurrency(input);IPendleCurrency dst=IPendleCurrency(output);
        uint256 spentBefore=src.balanceOf(msg.sender);uint256 beforeOut=dst.balanceOf(recipient);
        uint256 own=src.balanceOf(address(this));uint256 out=dst.balanceOf(address(this));
        uint256 sy0=IPendleCurrency(sy).balanceOf(address(this));uint256 yt0=IPendleCurrency(yt).balanceOf(address(this));
        if(src.allowance(address(this),address(router))!=0) revert Payment();
        _call(input,abi.encodeCall(src.transferFrom,(msg.sender,address(this),amount)));
        if(src.balanceOf(address(this))-own!=amount)revert Payment();
        _call(input,abi.encodeCall(src.approve,(address(router),amount)));
        IPendleWalletRouter.Terminal memory term;
        term.token=buy?input:output;term.syToken=term.token;term.amount=buy?amount:minimum;
        if(input==sy)router.swapExactSyForPtSimple(recipient,market,amount,minimum);
        else if(buy)router.swapExactTokenForPtSimple(recipient,market,minimum,term);
        else {
            IPendleWalletRouter.Limit memory limit;
            if(output==sy)router.swapExactPtForSy(recipient,market,amount,minimum,limit);
            else router.swapExactPtForToken(recipient,market,amount,term,limit);
        }
        if(src.allowance(address(this),address(router))!=0) _call(input,abi.encodeCall(src.approve,(address(router),0)));
        received=dst.balanceOf(recipient)-beforeOut;
        if(received<minimum || spentBefore-src.balanceOf(msg.sender)!=amount || src.balanceOf(address(this))!=own
            || dst.balanceOf(address(this))!=out || src.allowance(address(this),address(router))!=0
            || IPendleCurrency(sy).balanceOf(address(this))!=sy0 || IPendleCurrency(yt).balanceOf(address(this))!=yt0) revert Payment();
        emit Settled(msg.sender,recipient,market,input,output,amount,received);busy=false;
    }
    function _call(address token,bytes memory data) private {
        (bool ok,bytes memory res)=token.call(data);
        if(!ok || (res.length!=0 && !abi.decode(res,(bool))))revert Payment();
    }
}
