// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Один платёж двумя настоящими обменами: bopAMM WETH/USDC и указанный пул Uniswap V3 USDC/USDT.
// Проверяется исключительно в архивном eth_simulateV1. Контракт в реестре исполняемых модулей не регистрируется.
interface ICashToken {
    function balanceOf(address user) external view returns(uint256);
    function transferFrom(address owner,address to,uint256 value) external returns(bool);
    function transfer(address to,uint256 value) external returns(bool);
    function approve(address spender,uint256 value) external returns(bool);
    function allowance(address user,address spender) external view returns(uint256);
}
interface IBopCashLeg {
    struct Order {bytes32 leg;bytes32 parent;uint64 height;uint64 timestamp;address input;address output;uint256 amount;uint256 minimum;}
    function execute(Order calldata order) external returns(uint256,uint256);
}
interface IV3CashLeg {
    struct ExactInputSingleParams {address tokenIn;address tokenOut;uint24 fee;address recipient;uint256 deadline;uint256 amountIn;uint256 amountOutMinimum;uint160 sqrtPriceLimitX96;}
    function exactInputSingle(ExactInputSingleParams calldata param) external payable returns(uint256);
}
contract BopCashPathProbe {
    address constant WETH=0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2;
    address constant USDC=0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48;
    address constant USDT=0xdAC17F958D2ee523a2206206994597C13D831ec7;
    address constant UNI=0xE592427A0AEce92De3Edee1F18E0157C05861564;
    IBopCashLeg immutable bop;
    bool transient used;
    constructor(IBopCashLeg module){bop=module;}
    function invoke(address token,bytes memory data) private {
        (bool ok,bytes memory ret)=token.call(data);
        require(ok && (ret.length==0 || abi.decode(ret,(bool))));
    }
    function stable(address a,address b,uint256 amount,uint24 fee) private returns(uint256 received){
        invoke(a,abi.encodeCall(ICashToken.approve,(UNI,amount)));
        received=IV3CashLeg(UNI).exactInputSingle(IV3CashLeg.ExactInputSingleParams(a,b,fee,address(this),block.timestamp,amount,1,0));
        require(ICashToken(a).allowance(address(this),UNI)==0 && received>0);
    }
    function maker(address a,address b,uint256 amount,IBopCashLeg.Order memory o) private returns(uint256 received){
        o.input=a;o.output=b;o.amount=amount;o.minimum=1;
        invoke(a,abi.encodeCall(ICashToken.approve,(address(bop),amount)));
        (uint256 paid,uint256 out)=bop.execute(o);
        require(ICashToken(a).allowance(address(this),address(bop))==0 && paid==amount && out>0);
        return out;
    }
    function execute(IBopCashLeg.Order calldata o,uint24 fee) external returns(uint256) {
        require(!used && block.chainid==1 && o.minimum>0 && o.amount>0);
        used=true;
        bool buy=o.input==USDT && o.output==WETH;
        require(buy || (o.input==WETH && o.output==USDT));
        uint256 beforeSrc=ICashToken(o.input).balanceOf(msg.sender);
        uint256 beforeDst=ICashToken(o.output).balanceOf(msg.sender);
        uint256 beforeU=ICashToken(USDC).balanceOf(address(this));
        uint256 beforeA=ICashToken(o.input).balanceOf(address(this));
        uint256 beforeB=ICashToken(o.output).balanceOf(address(this));
        invoke(o.input,abi.encodeCall(ICashToken.transferFrom,(msg.sender,address(this),o.amount)));
        uint256 x=buy?stable(USDT,USDC,o.amount,fee):maker(WETH,USDC,o.amount,o);
        require(ICashToken(USDC).balanceOf(address(this))==beforeU+x);
        uint256 net=buy?maker(USDC,WETH,x,o):stable(USDC,USDT,x,fee);
        require(net>=o.minimum && ICashToken(o.output).balanceOf(address(this))==beforeB+net &&
            ICashToken(USDC).balanceOf(address(this))==beforeU && ICashToken(o.input).balanceOf(address(this))==beforeA);
        invoke(o.output,abi.encodeCall(ICashToken.transfer,(msg.sender,net)));
        require(ICashToken(o.input).balanceOf(msg.sender)+o.amount==beforeSrc &&
            ICashToken(o.output).balanceOf(msg.sender)==beforeDst+net && ICashToken(o.output).balanceOf(address(this))==beforeB);
        used=false;return net;
    }
}
