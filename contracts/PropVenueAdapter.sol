// SPDX-License-Identifier: MIT
// Модуль владельца связывает выплату кошельку с фактически выбранной площадкой публичного PropAMMRouter.
// Обычный маршрут с резервным Uniswap V3 остаётся самостоятельным вариантом исполнения.
pragma solidity 0.8.37;
import {IBalancerVault} from "./IPools.sol";

interface IPropToken {
    function balanceOf(address account) external view returns(uint256);
    function allowance(address owner,address spender) external view returns(uint256);
    function transferFrom(address owner,address to,uint256 amount) external returns(bool);
    function approve(address spender,uint256 amount) external returns(bool);
}
interface IPublicPropRouter {
    function isWhitelistedVenue(address venue) external view returns(bool);
    function paused() external view returns(bool);
    function fallbackSwapRouter() external view returns(address);
    function swapViaVenueV1(address venue,address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        external returns(uint256 amountOut,address executedVenue);
}
contract PropVenueAdapter {
    IPublicPropRouter public immutable router;
    address public immutable market;
    address public immutable vault;
    bytes32 public immutable routerHash;
    bytes32 public immutable marketHash;
    bytes32 public immutable vaultHash;
    bool transient private busy;
    error InvalidOrder();
    error Payment();
    event Filled(address indexed payer,address indexed recipient,address indexed venue,address input,address output,uint256 spent,uint256 received);
    constructor(IPublicPropRouter publicRouter,address selected,address cashVault,bytes32 shell,bytes32 venueCode,bytes32 cashCode) {
        if(address(publicRouter).code.length==0||selected.code.length==0||cashVault.code.length==0||
            address(publicRouter)==selected||address(publicRouter)==cashVault||
            address(publicRouter).codehash!=shell||selected.codehash!=venueCode||cashVault.codehash!=cashCode) revert InvalidOrder();
        router=publicRouter; market=selected; vault=cashVault;
        routerHash=shell; marketHash=venueCode; vaultHash=cashCode;
    }
    function execute(address venue,address input,address output,uint256 amount,uint256 minimum,uint256 deadline)
        external returns(uint256) {
        if(minimum==0) revert InvalidOrder();
        return _trade(venue,input,output,amount,minimum,msg.sender,deadline,address(0));
    }
    function executeWithFallback(address venue,address input,address output,uint256 amount,uint256 minimum,uint256 deadline,address fallbackRouter)
        external returns(uint256) {
        if(minimum==0||fallbackRouter==address(0)||fallbackRouter!=router.fallbackSwapRouter()) revert InvalidOrder();
        return _trade(venue,input,output,amount,minimum,msg.sender,deadline,fallbackRouter);
    }
    function swap(IBalancerVault.SingleSwap calldata o,IBalancerVault.FundManagement calldata f,uint256 minimum,uint256 deadline)
        external payable returns(uint256) {
        if(msg.value!=0||o.kind!=0||o.userData.length!=0||f.sender!=msg.sender||f.fromInternalBalance||f.toInternalBalance||
            uint256(o.poolId)>type(uint160).max) revert InvalidOrder();
        return _trade(address(uint160(uint256(o.poolId))),o.assetIn,o.assetOut,o.amount,minimum,f.recipient,deadline,address(0));
    }
    function move(address token,bytes memory callData) private {
        (bool ok,bytes memory result)=token.call(callData);
        if(!ok||(result.length!=0&&!abi.decode(result,(bool)))) revert Payment();
    }
    function _trade(address venue,address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline,address fallbackRouter)
        private returns(uint256 net) {
        if(busy||amount==0||block.timestamp>deadline||input==output||input==address(0)||output==address(0)||
            recipient==address(0)||recipient==address(this)||venue==address(0)||venue==router.fallbackSwapRouter()||
            venue!=market||recipient==vault||msg.sender==vault||address(router).codehash!=routerHash||
            market.codehash!=marketHash||vault.codehash!=vaultHash||!router.isWhitelistedVenue(venue)||router.paused()) revert InvalidOrder();
        busy=true;
        IPropToken coin=IPropToken(input);
        uint256 ownerBefore=coin.balanceOf(msg.sender);
        uint256 balanceBefore=coin.balanceOf(address(this));
        uint256 outBefore=IPropToken(output).balanceOf(recipient);
        if(coin.allowance(address(this),address(router))!=0) revert Payment();
        move(input,abi.encodeCall(coin.transferFrom,(msg.sender,address(this),amount)));
        uint256 funded=coin.balanceOf(address(this))-balanceBefore;
        if(funded==0||funded>amount) revert Payment();
        move(input,abi.encodeCall(coin.approve,(address(router),funded)));
        uint256 vaultIn=coin.balanceOf(vault);
        uint256 vaultOut=IPropToken(output).balanceOf(vault);
        (uint256 quoteNet,address filled)=router.swapViaVenueV1(venue,input,output,funded,minimum,recipient,deadline);
        if(filled!=venue&&(fallbackRouter==address(0)||filled!=fallbackRouter)) revert Payment();
        if(coin.allowance(address(this),address(router))!=0) move(input,abi.encodeCall(coin.approve,(address(router),0)));
        net=IPropToken(output).balanceOf(recipient)-outBefore;
        if(filled==venue&&(coin.balanceOf(vault)-vaultIn!=funded||vaultOut-IPropToken(output).balanceOf(vault)!=net)) revert Payment();
        if(net==0||net<minimum||net!=quoteNet||coin.balanceOf(address(this))!=balanceBefore||
            ownerBefore-coin.balanceOf(msg.sender)!=amount||coin.allowance(address(this),address(router))!=0) revert Payment();
        emit Filled(msg.sender,recipient,filled,input,output,amount,net);
        busy=false;
    }
}
