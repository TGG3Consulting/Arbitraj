// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

import {BopAmmWallet, IBopMarket} from "./BopAmmWallet.sol";

interface IBopCashToken {
    function allowance(address owner,address spender) external view returns(uint256);
    function balanceOf(address owner) external view returns(uint256);
    function transfer(address to,uint256 amount) external returns(bool);
    function approve(address spender,uint256 amount) external returns(bool);
    function transferFrom(address from,address to,uint256 amount) external returns(bool);
}
interface IBopV3 {
    struct Swap {address input;address output;uint24 fee;address recipient;uint256 deadline;uint256 amount;uint256 minimum;uint160 priceLimit;}
    function exactInputSingle(Swap calldata swap) external payable returns(uint256);
}
interface IBopCurve {
    function quote(address input,address output,uint256 amount) external view returns(uint256);
}
interface IBopV3Quote {
    function quoteExactInputSingle(address a,address b,uint24 fee,uint256 amount,uint160 limit) external returns(uint256);
}

/// @notice Wallet-owned approvals cover the full terminal input. Each order is valid for one Ethereum slot.
contract BopCashWallet is BopAmmWallet {
    address public constant WETH=0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2;
    address public constant USDC=0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48;
    address public constant USDT=0xdAC17F958D2ee523a2206206994597C13D831ec7;
    address public constant V3_ROUTER=0xE592427A0AEce92De3Edee1F18E0157C05861564;
    address public constant V3_QUOTER=0xb27308f9F90D607463bb33eA1BeBb41C27CE5AB6;
    uint24 public constant POOL_FEE=100;
    bool transient private cashBusy;
    event CashFilled(address indexed payer,bytes32 indexed leg,address indexed input,address output,uint256 spent,uint256 intermediate,uint256 received);

    constructor(IBopMarket venue,address expectedPricing,address expectedUsdc) BopAmmWallet(venue,expectedPricing,expectedUsdc) {
        if(expectedUsdc!=USDC) revert InvalidMarket();
    }

    function buying(address a,address b) private pure returns(bool) {
        if(a==USDT && b==WETH) return true;
        if(a!=WETH || b!=USDT) revert InvalidOrder();
        return false;
    }

    function quoteRoute(address input,address output,uint256 amount,bool bridge) external returns(uint256) {
        bool buy=buying(input,output);
        IBopCurve named=IBopCurve(address(market));
        if(!bridge) return named.quote(input,output,amount);
        IBopV3Quote quoter=IBopV3Quote(V3_QUOTER);
        if(buy) return named.quote(USDC,WETH,quoter.quoteExactInputSingle(USDT,USDC,POOL_FEE,amount,0));
        return quoter.quoteExactInputSingle(USDC,USDT,POOL_FEE,named.quote(WETH,USDC,amount),0);
    }

    function move(address token,bytes memory args) private {
        (bool ok,bytes memory ret)=token.call(args);
        if(!ok || (ret.length!=0 && (ret.length!=32 || !abi.decode(ret,(bool))))) revert InvalidFill();
    }

    function stable(address a,address b,uint256 amount) private returns(uint256) {
        move(a,abi.encodeCall(IBopCashToken.approve,(V3_ROUTER,amount)));
        uint256 out=IBopV3(V3_ROUTER).exactInputSingle(IBopV3.Swap(a,b,POOL_FEE,address(this),block.timestamp,amount,1,0));
        if(IBopCashToken(a).allowance(address(this),V3_ROUTER)!=0 || out==0) revert InvalidFill();
        return out;
    }

    function maker(Order memory o,address a,address b,uint256 amount) private returns(uint256) {
        o.input=a;o.output=b;o.amount=amount;o.minimum=1;
        o.leg=keccak256(abi.encode(msg.sender,o.leg));
        move(a,abi.encodeCall(IBopCashToken.approve,(address(this),amount)));
        (uint256 paid,uint256 received)=this.execute(o);
        // WETH9 skips allowance consumption when the sender and spender coincide.
        if(IBopCashToken(a).allowance(address(this),address(this))!=0)
            move(a,abi.encodeCall(IBopCashToken.approve,(address(this),0)));
        if(paid!=amount || received==0 || IBopCashToken(a).allowance(address(this),address(this))!=0) revert InvalidFill();
        return received;
    }

    function executeCash(Order calldata o) external returns(uint256 spent,uint256 received) {
        if(cashBusy || o.leg==bytes32(0) || completed[msg.sender][o.leg] || o.minimum==0 || o.amount==0) revert InvalidOrder();
        bool buy=buying(o.input,o.output);
        cashBusy=true;
        completed[msg.sender][o.leg]=true;
        IBopCashToken input=IBopCashToken(o.input);
        IBopCashToken output=IBopCashToken(o.output);
        IBopCashToken intermediate=IBopCashToken(USDC);
        uint256 ownInput=input.balanceOf(msg.sender);
        uint256 ownOutput=output.balanceOf(msg.sender);
        uint256 initialInput=input.balanceOf(address(this));
        uint256 initialOutput=output.balanceOf(address(this));
        uint256 initialUSDC=intermediate.balanceOf(address(this));
        move(o.input,abi.encodeCall(input.transferFrom,(msg.sender,address(this),o.amount)));
        if(input.balanceOf(address(this))!=initialInput+o.amount) revert InvalidFill();
        uint256 mid=buy?stable(USDT,USDC,o.amount):maker(o,WETH,USDC,o.amount);
        if(intermediate.balanceOf(address(this))!=initialUSDC+mid) revert InvalidFill();
        uint256 gross=buy?maker(o,USDC,WETH,mid):stable(USDC,USDT,mid);
        if(output.balanceOf(address(this))!=initialOutput+gross || input.balanceOf(address(this))!=initialInput ||
            intermediate.balanceOf(address(this))!=initialUSDC) revert InvalidFill();
        move(o.output,abi.encodeCall(output.transfer,(msg.sender,gross)));
        spent=ownInput-input.balanceOf(msg.sender);
        received=output.balanceOf(msg.sender)-ownOutput;
        if(spent!=o.amount || received<o.minimum || output.balanceOf(address(this))!=initialOutput) revert InvalidFill();
        emit CashFilled(msg.sender,o.leg,o.input,o.output,spent,mid,received);
        cashBusy=false;
    }
}
