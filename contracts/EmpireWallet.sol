// SPDX-License-Identifier: MIT
// Владелец может разместить модуль одной фабрики Empire Ethereum и указать адрес в настройках.
// execute списывает полный вход кошелька. Шаг 29 V5 вызывает swapFrom и проверяет минимум всей операции.
pragma solidity 0.8.37;

interface IEmpireRegistry {
    function getPair(address tokenA,address tokenB) external view returns (address);
}
interface IEmpireMarket {
    function factory() external view returns (address);
    function token0() external view returns (address);
    function token1() external view returns (address);
    function getReserves() external view returns (uint112,uint112,uint32);
    function sweepableToken() external view returns (address);
    function sweptAmount() external view returns (uint256);
    function swap(uint256 amount0Out,uint256 amount1Out,address recipient,bytes calldata data) external;
}
interface IEmpireCoin {
    function balanceOf(address account) external view returns (uint256);
    function transferFrom(address payer,address recipient,uint256 amount) external returns (bool);
}

contract EmpireWallet {
    // EmpirePair.swap, balance0Adjusted/balance1Adjusted at
    // https://eth.blockscout.com/address/0x163b890f4892d593945579028ec44ef0f20a4633?tab=contract
    uint256 private constant FEE_UNITS=3;
    uint256 private constant FEE_SCALE=1000;
    IEmpireRegistry public immutable factory;
    bool transient private entered;
    error InvalidOrder();
    error InsufficientBacking();
    error ChangedBalance();
    event Filled(address indexed pool,address indexed payer,address indexed recipient,address input,address output,
                 uint256 debit,uint256 credit,uint256 gross,uint256 received);

    constructor(IEmpireRegistry registry) {
        if(block.chainid!=1 || address(registry).code.length==0) revert InvalidOrder();
        factory=registry;
    }

    function execute(address pool,address input,address output,uint256 amount,uint256 minimum,uint256 deadline)
        external returns(uint256) {
        if(minimum==0) revert InvalidOrder();
        return settle(pool,input,output,amount,minimum,msg.sender,deadline);
    }

    function swapFrom(address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        external returns(uint256) {
        return settle(factory.getPair(input,output),input,output,amount,minimum,recipient,deadline);
    }

    function settle(address pool,address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        private returns(uint256 received) {
        if(entered || block.timestamp>deadline || input==output || amount==0 || recipient==address(0)
            || recipient==pool || recipient==address(this) || msg.sender==pool) revert InvalidOrder();
        entered=true;
        IEmpireMarket pair=IEmpireMarket(pool);
        bool zero=input<output;
        if(pool==address(0) || pool!=factory.getPair(input,output) || pair.factory()!=address(factory)
            || pair.token0()!=(zero?input:output) || pair.token1()!=(zero?output:input)) revert InvalidOrder();
        IEmpireCoin coin=IEmpireCoin(input);
        IEmpireCoin payout=IEmpireCoin(output);
        uint256 initial=coin.balanceOf(msg.sender);
        uint256 start=payout.balanceOf(recipient);
        uint256 physical=coin.balanceOf(pool);
        (bool ok,bytes memory ret)=input.call(abi.encodeCall(IEmpireCoin.transferFrom,(msg.sender,pool,amount)));
        if(!ok || (ret.length!=0 && !abi.decode(ret,(bool)))) revert ChangedBalance();
        uint256 funded=coin.balanceOf(pool);
        uint256 delta=funded-physical;
        uint256 credit=delta<amount?delta:amount;
        (uint256 r0,uint256 r1,)=pair.getReserves();
        (uint256 rin,uint256 rout)=zero?(r0,r1):(r1,r0);
        uint256 logical=funded+(pair.sweepableToken()==input?pair.sweptAmount():0);
        if(logical<=rin || logical>type(uint112).max || rin==0) revert InsufficientBacking();
        if(credit>logical-rin) credit=logical-rin;
        uint256 net=credit*(FEE_SCALE-FEE_UNITS);
        uint256 gross=net*rout/(rin*FEE_SCALE+net);
        if(gross==0 || gross>payout.balanceOf(pool)) revert InsufficientBacking();
        pair.swap(zero?0:gross,zero?gross:0,recipient,"");
        received=payout.balanceOf(recipient)-start;
        if(initial-coin.balanceOf(msg.sender)!=amount || received==0 || received<minimum) revert ChangedBalance();
        emit Filled(pool,msg.sender,recipient,input,output,amount,credit,gross,received);
        entered=false;
    }
}
