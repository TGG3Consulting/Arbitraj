// SPDX-License-Identifier: MIT
// Обмен ERC-20 от плательщика через зарегистрированную пару Memebox по её доступным резервам.
pragma solidity 0.8.37;

interface IMemeboxFactory {
    function getPair(address a, address b) external view returns (address);
}

interface IMemeboxPair {
    function factory() external view returns (address);
    function token0() external view returns (address);
    function token1() external view returns (address);
    function getReserves() external view returns (uint112, uint112, uint32);
    function balances() external view returns (uint256, uint256);
    function poolFee() external view returns (uint16);
    function protocolRatio() external view returns (uint16);
    function swap(uint256 amount0Out,uint256 amount1Out,address recipient,bytes calldata data) external;
}

interface IMemeboxToken {
    function balanceOf(address owner) external view returns (uint256);
    function transferFrom(address from,address to,uint256 amount) external returns (bool);
}

contract MemeboxAdapter {
    IMemeboxFactory public immutable factory;
    bool transient private entered;

    constructor(IMemeboxFactory registry) {
        require(address(registry).code.length>0,"factory");
        factory=registry;
    }

    function market(address a,address b) private view returns (IMemeboxPair p,bool zeroIn) {
        zeroIn=a<b;
        p=IMemeboxPair(factory.getPair(a,b));
        require(a!=b && address(p)!=address(0) && p.factory()==address(factory),"pair");
        require(p.token0()==(zeroIn?a:b) && p.token1()==(zeroIn?b:a),"tokens");
    }

    function reserves(IMemeboxPair p,bool zeroIn) private view returns (uint256 rin,uint256 rout) {
        (uint112 x,uint112 y,)=p.getReserves();
        (rin,rout)=zeroIn?(x,y):(y,x);
    }

    function accounted(IMemeboxPair p,bool zeroIn) private view returns (uint256,uint256) {
        (uint256 x,uint256 y)=p.balances();
        return zeroIn?(x,y):(y,x);
    }

    function quote(uint256 rin,uint256 rout,uint256 paidIn,uint256 liveOut,uint256 fee) private pure returns (uint256) {
        require(rin!=0 && rout>1 && fee<=10_000 && paidIn>rin,"reserves");
        uint256 input=paidIn-rin;
        uint256 debit=input/10_000*fee+(input%10_000)*fee/10_000;
        uint256 adjusted=paidIn-debit;
        require(adjusted<=type(uint112).max,"overflow");
        uint256 invariant=rin*rout;
        uint256 keep=(invariant-1)/adjusted+1;
        require(liveOut>keep,"empty");
        uint256 out=liveOut-keep;
        if (out>=rout) out=rout-1;
        require(liveOut-out<=type(uint112).max,"overflow");
        return out;
    }

    // Сумма здесь — чистый приход на пару после возможного налога входного токена.
    function getAmountOut(address a,address b,uint256 credit) external view returns (uint256) {
        (IMemeboxPair p,bool z)=market(a,b);
        (uint256 i,uint256 o)=reserves(p,z);
        (uint256 liveIn,uint256 liveOut)=accounted(p,z);
        require(p.protocolRatio()<=10_000,"ratio");
        return quote(i,o,liveIn+credit,liveOut,p.poolFee());
    }

    function swapFrom(address a,address b,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        external returns(uint256 received) {
        return settle(a,b,amount,minimum,recipient,deadline);
    }

    // Штатный прямой ERC-20 вызов кошелька; выход всегда получает исходный плательщик.
    function execute(address pool,address a,address b,uint256 amount,uint256 minimum,uint256 deadline)
        external returns(uint256 received) {
        require(pool!=address(0) && pool==factory.getPair(a,b),"market");
        return settle(a,b,amount,minimum,msg.sender,deadline);
    }

    function settle(address a,address b,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        private returns(uint256 received) {
        require(!entered && amount>0 && block.timestamp<=deadline,"order");
        entered=true;
        (IMemeboxPair p,bool z)=market(a,b);
        require(recipient!=address(0) && recipient!=address(p) && recipient!=address(this),"recipient");
        (uint256 i,uint256 o)=reserves(p,z);
        (uint256 liveIn,uint256 liveOut)=accounted(p,z);
        require(p.protocolRatio()<=10_000,"ratio");
        uint256 paid=IMemeboxToken(a).balanceOf(msg.sender);
        uint256 beforePair=IMemeboxToken(a).balanceOf(address(p));
        uint256 beforeOut=IMemeboxToken(b).balanceOf(recipient);
        (bool ok,bytes memory ret)=a.call(abi.encodeCall(IMemeboxToken.transferFrom,(msg.sender,address(p),amount)));
        require(ok && (ret.length==0 || abi.decode(ret,(bool))),"transfer");
        uint256 credit=IMemeboxToken(a).balanceOf(address(p))-beforePair;
        require(credit>0 && paid-IMemeboxToken(a).balanceOf(msg.sender)==amount,"spent");
        (uint256 afterIn,uint256 afterOut)=accounted(p,z);
        (uint256 nextI,uint256 nextO)=reserves(p,z);
        require(i==nextI && o==nextO && afterIn==liveIn+credit && afterOut==liveOut,"changed");
        uint256 gross=quote(i,o,afterIn,afterOut,p.poolFee());
        require(gross!=0,"empty");
        p.swap(z?0:gross,z?gross:0,recipient,"");
        received=IMemeboxToken(b).balanceOf(recipient)-beforeOut;
        require(received>0 && received>=minimum,"payout");
        entered=false;
    }
}
