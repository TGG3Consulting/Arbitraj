// SPDX-License-Identifier: MIT
// Модуль обменивает ERC20 плательщика через проверенную пару CandySwap на Base.
// Владелец развёртывает модуль с адресом фабрики. Кошелёк вызывает execute напрямую
// или через шаг 29 V5 вызывает swapFrom. Выплату ограничивает собственный взнос плательщика.
pragma solidity 0.8.37;

interface ICandyRegistry {
    function getPair(address a,address b) external view returns(address);
    function feeTo() external view returns(address);
}
interface ICandyPair {
    function factory() external view returns(address);
    function token0() external view returns(address);
    function token1() external view returns(address);
    function getReserves() external view returns(uint112,uint112,uint32);
    function swapFee() external view returns(uint256);
    function swap(uint256 amount0Out,uint256 amount1Out,address to,bytes calldata data) external;
}
interface ICandyCoin {
    function balanceOf(address who) external view returns(uint256);
    function transferFrom(address from,address to,uint256 amount) external returns(bool);
}

contract CandySwapAdapter {
    ICandyRegistry public immutable factory;
    bool transient private entered;
    error InvalidOrder();
    error ChangedMarket();
    error InsufficientOutput();

    constructor(ICandyRegistry registry) {
        if(address(registry).code.length==0) revert InvalidOrder();
        factory=registry;
    }

    function grossOut(uint256 ri,uint256 ro,uint256 physical,uint256 credit,uint256 fee) private pure returns(uint256) {
        if(physical<=ri || physical>type(uint112).max || ri==0 || fee>=10_000) revert ChangedMarket();
        if(credit>physical-ri) credit=physical-ri;
        uint256 net=credit*(10_000-fee);
        return net*ro/(ri*10_000+net);
    }

    function getAmountOut(address input,address output,uint256 credit) external view returns(uint256) {
        ICandyPair p=ICandyPair(factory.getPair(input,output));
        bool z=input==p.token0();
        if(input==output || p.factory()!=address(factory)
            || (z?p.token1()!=output:(p.token1()!=input || p.token0()!=output))) revert ChangedMarket();
        (uint112 x,uint112 y,)=p.getReserves();
        (uint256 ri,uint256 ro)=z?(x,y):(y,x);
        if(ICandyCoin(output).balanceOf(address(p))<ro) revert ChangedMarket();
        uint256 gross=grossOut(ri,ro,ICandyCoin(input).balanceOf(address(p))+credit,credit,p.swapFee());
        (bool ok,bytes memory data)=address(p).staticcall(hex"c3595a85");
        if(!ok || data.length!=32) revert ChangedMarket();
        uint256 rate=abi.decode(data,(uint256));
        if(rate>10_000) revert ChangedMarket();
        return gross-gross*rate/10_000;
    }

    function swapFrom(address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        external returns(uint256) {
        return settle(factory.getPair(input,output),input,output,amount,minimum,recipient,deadline);
    }

    function execute(address market,address input,address output,uint256 amount,uint256 minimum,uint256 deadline)
        external returns(uint256) {
        if(market!=factory.getPair(input,output)) revert ChangedMarket();
        return settle(market,input,output,amount,minimum,msg.sender,deadline);
    }

    function settle(address market,address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        private returns(uint256 received) {
        if(entered || block.timestamp>deadline || amount==0 || recipient==address(0)
            || recipient==market || recipient==address(this) || recipient==factory.feeTo()) revert InvalidOrder();
        entered=true;
        ICandyPair pair=ICandyPair(market);
        bool zeroIn=input==pair.token0();
        if(input==output || pair.factory()!=address(factory)
            || (zeroIn?pair.token1()!=output:(pair.token1()!=input || pair.token0()!=output))) revert ChangedMarket();

        (uint112 r0,uint112 r1,)=pair.getReserves();
        (uint256 ri,uint256 ro)=zeroIn?(r0,r1):(r1,r0);
        ICandyCoin incoming=ICandyCoin(input);
        ICandyCoin outgoing=ICandyCoin(output);
        uint256 initial=incoming.balanceOf(market);
        uint256 held=outgoing.balanceOf(market);
        uint256 beforeIn=incoming.balanceOf(msg.sender);
        uint256 beforeOut=outgoing.balanceOf(recipient);
        if(ri==0 || held<ro) revert ChangedMarket();
        (bool ok,bytes memory result)=input.call(abi.encodeCall(incoming.transferFrom,(msg.sender,market,amount)));
        if(!ok || (result.length!=0 && !abi.decode(result,(bool))) || beforeIn-incoming.balanceOf(msg.sender)!=amount)
            revert InvalidOrder();
        uint256 physical=incoming.balanceOf(market);
        (uint112 keep0,uint112 keep1,)=pair.getReserves();
        if(keep0!=r0 || keep1!=r1 || outgoing.balanceOf(market)!=held || physical>type(uint112).max)
            revert ChangedMarket();
        uint256 own=physical-initial;
        if(own>amount) own=amount;
        uint256 gross=grossOut(ri,ro,physical,own,pair.swapFee());
        if(gross==0) revert InsufficientOutput();
        pair.swap(zeroIn?0:gross,zeroIn?gross:0,recipient,"");
        received=outgoing.balanceOf(recipient)-beforeOut;
        if(received<minimum || received==0) revert InsufficientOutput();
        entered=false;
    }
}
