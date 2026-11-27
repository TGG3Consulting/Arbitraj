// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Кошелёк вызывает только исходный exchange/exchange_dy нужного LLAMMA из реестра Curve.
// Проверяются физические валюты, свой предельный расход, чистая выплата, возврат и deadline.
// ABI обоих поколений: https://docs.curve.finance/developer/integration/llamma/
// V2 market tuple: https://github.com/curvefi/curve-stablecoin/blob/master/curve_stablecoin/lending/LendFactory.vy
interface ILlamaFactory {
    function amms(uint256 i) external view returns(address);
    function controllers(uint256 i) external view returns(address);
    function markets(uint256 i) external view returns(address,address,address,address,address,address,address);
}
interface ILlamaPool {
    function admin() external view returns(address);
    function coins(uint256 i) external view returns(address);
    function exchange(uint256 i,uint256 j,uint256 amount,uint256 minimum,address recipient) external returns(uint256[2] memory);
    function exchange_dy(uint256 i,uint256 j,uint256 amount,uint256 maximum,address recipient) external returns(uint256[2] memory);
}
interface ILlamaController { function amm() external view returns(address); }
interface ILlamaToken {
    function balanceOf(address account) external view returns(uint256);
    function allowance(address account,address spender) external view returns(uint256);
    function transfer(address account,uint256 amount) external returns(bool);
    function transferFrom(address source,address account,uint256 amount) external returns(bool);
    function approve(address account,uint256 amount) external returns(bool);
}

contract LlammaWallet {
    ILlamaFactory public immutable factory;
    bytes32 public immutable factoryHash;
    bool public immutable tupleRegistry;
    bool private busy;
    struct Order {
        uint256 index;
        address pool;
        bytes32 poolHash;
        address input;
        address output;
        uint256 maximum;
        uint256 minimum;
        uint256 deadline;
        bool byTarget;
    }
    event Paid(address indexed payer,address indexed pool,address indexed input,address output,uint256 spent,uint256 received);
    error Market();
    error Bounds();
    error Payment();

    constructor(ILlamaFactory registrar,bool tuples) {
        if(address(registrar).code.length==0) revert Market();
        factory=registrar;
        factoryHash=address(registrar).codehash;
        tupleRegistry=tuples;
    }
    function side(Order calldata o) private view returns(uint256){
        if(address(factory).codehash!=factoryHash||o.pool.code.length==0||o.pool.codehash!=o.poolHash) revert Market();
        address pool;
        address controller;
        if(tupleRegistry) ( ,controller,pool,,,,)=factory.markets(o.index);
        else { pool=factory.amms(o.index); controller=factory.controllers(o.index); }
        if(o.pool!=pool||ILlamaPool(pool).admin()!=controller||ILlamaController(controller).amm()!=pool) revert Market();
        address zero=ILlamaPool(pool).coins(0);
        address one=ILlamaPool(pool).coins(1);
        if(o.input==o.output || zero==address(0)||one==address(0)) revert Market();
        if(o.input==zero&&o.output==one) return 0;
        if(o.input==one&&o.output==zero) return 1;
        revert Market();
    }
    function pay(address token,bytes memory data) private {
        (bool ok,bytes memory ret)=token.call(data);
        if(!ok||(ret.length!=0&&!abi.decode(ret,(bool)))) revert Payment();
    }
    function execute(Order calldata o) external returns(uint256 spent,uint256 received){
        if(busy||block.timestamp>o.deadline||o.maximum==0||o.minimum==0) revert Bounds();
        busy=true;
        uint256 i=side(o);
        ILlamaToken src=ILlamaToken(o.input);
        ILlamaToken dst=ILlamaToken(o.output);
        uint256 initial=src.balanceOf(address(this));
        uint256 initialOutput=dst.balanceOf(address(this));
        uint256 ownerIn=src.balanceOf(msg.sender);
        uint256 ownerOut=dst.balanceOf(msg.sender);
        uint256 poolIn=src.balanceOf(o.pool);
        if(src.allowance(address(this),o.pool)!=0) revert Payment();
        pay(o.input,abi.encodeCall(ILlamaToken.transferFrom,(msg.sender,address(this),o.maximum)));
        if(src.balanceOf(address(this))-initial!=o.maximum) revert Payment();
        pay(o.input,abi.encodeCall(ILlamaToken.approve,(o.pool,o.maximum)));
        uint256[2] memory filled=o.byTarget?
            ILlamaPool(o.pool).exchange_dy(i,1-i,o.minimum,o.maximum,msg.sender):
            ILlamaPool(o.pool).exchange(i,1-i,o.maximum,o.minimum,msg.sender);
        uint256 change=src.balanceOf(address(this))-initial;
        if(change>o.maximum) revert Payment();
        spent=o.maximum-change;
        if(spent==0||spent!=filled[0]||src.balanceOf(o.pool)-poolIn!=spent) revert Payment();
        if(src.allowance(address(this),o.pool)!=0){
            pay(o.input,abi.encodeCall(ILlamaToken.approve,(o.pool,0)));
            if(src.allowance(address(this),o.pool)!=0) revert Payment();
        }
        if(change!=0) pay(o.input,abi.encodeCall(ILlamaToken.transfer,(msg.sender,change)));
        received=dst.balanceOf(msg.sender)-ownerOut;
        if(src.balanceOf(address(this))!=initial||dst.balanceOf(address(this))!=initialOutput||ownerIn-src.balanceOf(msg.sender)!=spent||
           received<o.minimum||filled[1]==0) revert Payment();
        emit Paid(msg.sender,o.pool,o.input,o.output,spent,received);
        busy=false;
    }
}

