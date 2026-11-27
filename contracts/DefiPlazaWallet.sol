// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Файл: DefiPlazaWallet.sol. Вызывается плательщиком или шагом 29 ArbExecutor V5.
// Проверяет срок, полный собственный расход и чистую выплату после обычного обмена.
// Конструктор фиксирует конкретную кассу, её поколение и официальный WETH.
interface IPlazaAsset {
    function balanceOf(address) external view returns (uint256);
    function allowance(address,address) external view returns (uint256);
    function transferFrom(address,address,uint256) external returns (bool);
    function transfer(address,uint256) external returns (bool);
    function approve(address,uint256) external returns (bool);
    function deposit() external payable;
    function withdraw(uint256) external;
}
interface IMultiPlaza {
    function DFPconfig() external view returns(bool,uint64,uint64);
    function listedTokens(address) external view returns(uint8,uint112);
    function swap(address,address,uint256,uint256) external payable returns(uint256);
}
interface IStablePlaza {
    function getIndex(address) external view returns(uint256);
    function getTokenFromIndex(uint256) external view returns(address);
    function getOutFromIn(uint256,uint256,uint256) external view returns(uint256);
    function easySwap(uint256,uint256,uint256,address) external returns(uint256);
}
contract DefiPlazaWallet {
    address public immutable exchange;
    address public immutable wrapped;
    bool public immutable stable;
    bytes32 public immutable executorHash;
    uint256 private constant STABLE_ASSETS=4;
    // DeFiPlaza.sol:20 enum State, :31 DFPconfig.oneMinusTradingFee (0.64).
    // https://sourcify.dev/server/v2/contract/1/0xE68c1d72340aEeFe5Be76eDa63AE2f4bc7514110?fields=sources
    uint8 private constant LISTED=3;
    uint256 private constant Q64_FRACTION_BITS=64;
    bool transient private busy;
    error Bounds();
    error Cash();
    event Filled(address indexed payer,address indexed recipient,address indexed input,address output,uint256 spent,uint256 received);

    constructor(address pool,address weth,bool stablePool,bytes32 approvedExecutorRuntime) {
        if(approvedExecutorRuntime==bytes32(0) || pool.code.length==0 || weth.code.length==0 || pool==weth) revert Bounds();
        exchange=pool;
        wrapped=weth;
        stable=stablePool;
        executorHash=approvedExecutorRuntime;
    }

    // MultiPlaza uses transfer with the EVM stipend for an ETH output.
    receive() external payable {}

    function balance(address token,address account) private view returns(uint256) {
        return token==address(0)?account.balance:IPlazaAsset(token).balanceOf(account);
    }
    function tokenCall(address token,bytes memory data) private {
        (bool ok,bytes memory result)=token.call(data);
        if(!ok || (result.length!=0 && !abi.decode(result,(bool)))) revert Cash();
    }
    function index(address token) private view returns(uint256 i) {
        i=IStablePlaza(exchange).getIndex(token);
        if(i>=STABLE_ASSETS || IStablePlaza(exchange).getTokenFromIndex(i)!=token) revert Bounds();
    }
    function physical(address asset) private view returns(address) {
        return !stable && asset==wrapped ? address(0) : asset;
    }
    function quote(address input,address output,uint256 amount) external view returns(uint256) {
        address src=physical(input);
        address dst=physical(output);
        if(src==dst || amount==0) revert Bounds();
        if(stable) return IStablePlaza(exchange).getOutFromIn(index(src),index(dst),amount);
        (bool open,uint64 keep,)=IMultiPlaza(exchange).DFPconfig();
        if(!open) revert Bounds();
        if(src!=address(0)){(uint8 state,)=IMultiPlaza(exchange).listedTokens(src);if(state!=LISTED)revert Bounds();}
        if(dst!=address(0)){(uint8 state,)=IMultiPlaza(exchange).listedTokens(dst);if(state!=LISTED)revert Bounds();}
        uint256 credited=amount*keep;
        return credited*balance(dst,exchange)/((balance(src,exchange)<<Q64_FRACTION_BITS)+credited);
    }
    function swapFrom(address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        external payable returns(uint256 received)
    {
        if(minimum==0 && msg.sender.codehash!=executorHash) revert Bounds();
        return settle(input,output,amount,minimum,recipient,deadline);
    }
    function execute(address input,address output,uint256 amount,uint256 minimum,uint256 deadline)
        external payable returns(uint256) {
        if(minimum==0) revert Bounds();
        return settle(input,output,amount,minimum,msg.sender,deadline);
    }
    function settle(address input,address output,uint256 amount,uint256 minimum,address recipient,uint256 deadline)
        private returns(uint256 received) {
        address src=physical(input);
        address dst=physical(output);
        if(busy || src==dst || amount==0 || block.timestamp>deadline || recipient==address(0) ||
            recipient==exchange || recipient==address(this) || msg.sender==exchange ||
            msg.value!=(input==address(0)?amount:0) || (stable && (src==address(0)||dst==address(0)))) revert Bounds();
        busy=true;
        uint256 oldIn=balance(input,address(this))-msg.value;
        uint256 oldOut=balance(output,address(this));
        uint256 oldEth=address(this).balance-msg.value;
        uint256 payer=balance(input,msg.sender)+msg.value;
        uint256 receiver=balance(output,recipient);
        uint256 beforeExchange=balance(src,exchange);
        if(input!=address(0)) {
            tokenCall(input,abi.encodeCall(IPlazaAsset.transferFrom,(msg.sender,address(this),amount)));
            if(balance(input,address(this))!=oldIn+amount) revert Cash();
        }
        if(src!=input) IPlazaAsset(wrapped).withdraw(amount);
        if(src!=address(0)) {
            if(IPlazaAsset(src).allowance(address(this),exchange)!=0) revert Cash();
            tokenCall(src,abi.encodeCall(IPlazaAsset.approve,(exchange,amount)));
        }
        uint256 beforeOutput=balance(dst,address(this));
        if(stable) {
            uint256 key=index(src) | (index(dst)<<8);
            IStablePlaza(exchange).easySwap(key,amount,0,address(this));
        } else {
            IMultiPlaza(exchange).swap{value:src==address(0)?amount:0}(src,dst,amount,0);
        }
        if(balance(src,exchange)!=beforeExchange+amount) revert Cash();
        if(src!=address(0) && IPlazaAsset(src).allowance(address(this),exchange)!=0) revert Cash();
        uint256 gross=balance(dst,address(this))-beforeOutput;
        if(dst!=output) IPlazaAsset(wrapped).deposit{value:gross}();
        if(output==address(0)){
            (bool ok,)=payable(recipient).call{value:gross}("");
            if(!ok) revert Cash();
        } else tokenCall(output,abi.encodeCall(IPlazaAsset.transfer,(recipient,gross)));
        received=balance(output,recipient)-receiver;
        if(received==0 || received<minimum || payer-balance(input,msg.sender)!=amount ||
            balance(input,address(this))!=oldIn || balance(output,address(this))!=oldOut ||
            address(this).balance!=oldEth) revert Cash();
        emit Filled(msg.sender,recipient,input,output,amount,received);
        busy=false;
    }
}

