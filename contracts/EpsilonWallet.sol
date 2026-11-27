// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Одиночная оплата Epsilon V6 через штатные V3 роутеры. Сверяет расход плательщика,
// чистую выдачу после перевода ERC-20 и сохранение прежнего имущества обоих исполнителей.
interface EpsToken {
    function balanceOf(address) external view returns (uint256);
    function allowance(address,address) external view returns (uint256);
    function approve(address,uint256) external returns (bool);
    function transfer(address,uint256) external returns (bool);
    function transferFrom(address,address,uint256) external returns (bool);
}
interface EpsRouter {
    struct Routes { address[] targets; bytes[] callDatas; uint256[] amounts; }
    struct TokenPermissions { address token; uint256 amount; }
    struct Permit { TokenPermissions permitted; uint256 nonce; uint256 deadline; }
    function protocolFeeBps() external view returns (uint16);
    function swap(address,address,uint256,uint256,uint256,bool,Routes calldata,
        Permit calldata,bytes calldata) external payable returns (uint256);
}
interface EpsProvider {
    struct Single {
        address tokenIn; address tokenOut; uint24 fee; address recipient;
        uint256 amountIn; uint256 amountOutMinimum; uint160 sqrtPriceLimitX96;
    }
    function exactInputSingle(Single calldata) external payable returns (uint256);
    function multicall(uint256,bytes[] calldata) external payable returns (bytes[] memory);
}
contract EpsilonWallet {
    error InvalidRoute();
    error Payment();
    address public immutable router;
    address public immutable first;
    address public immutable second;
    bytes32 public immutable routerHash;
    bytes32 public immutable firstHash;
    bytes32 public immutable secondHash;
    bool transient private entered;

    constructor(address venue, address a, address b) {
        if (venue.code.length==0 || a.code.length==0 || b.code.length==0 ||
            venue==a || venue==b || a==b) revert InvalidRoute();
        router=venue; first=a; second=b;
        routerHash=venue.codehash; firstHash=a.codehash; secondHash=b.codehash;
    }

    // Один или два разных провайдера. firstAmount — часть входа после Epsilon protocol fee.
    struct Order {
        address input; address output; uint256 amount; uint256 minimum; uint256 deadline;
        uint16 feeBps; uint24 firstTier; uint24 secondTier; uint256 firstAmount;
    }
    event Settled(address indexed payer, address indexed input, address indexed output,
        bytes32 orderHash, uint256 spent, uint256 received, uint256 protocolFee);

    function trade(Order calldata o) external returns(uint256 received) {
        if (entered || block.timestamp>o.deadline || o.input==o.output || o.amount==0 || o.minimum==0 ||
            router.codehash!=routerHash || first.codehash!=firstHash || second.codehash!=secondHash ||
            EpsRouter(router).protocolFeeBps()!=o.feeBps) revert InvalidRoute();
        entered=true;
        uint256 fee=o.amount*uint256(o.feeBps)/10000;
        uint256 net=o.amount-fee;
        if (net==0 || o.firstAmount>net) revert InvalidRoute();
        uint256 selfIn=balance(o.input,address(this));
        uint256 selfOut=balance(o.output,address(this));
        uint256 venueIn=balance(o.input,router);
        uint256 venueOut=balance(o.output,router);
        uint256 payerIn=balance(o.input,msg.sender);
        uint256 payerOut=balance(o.output,msg.sender);
        if (EpsToken(o.input).allowance(address(this),router)!=0) revert Payment();
        transfer(o.input,abi.encodeCall(EpsToken.transferFrom,(msg.sender,address(this),o.amount)));
        if (balance(o.input,address(this))!=selfIn+o.amount) revert Payment();
        transfer(o.input,abi.encodeCall(EpsToken.approve,(router,o.amount)));
        EpsRouter.Routes memory routes;
        uint256 count=o.firstAmount==0 || o.firstAmount==net ? 1 : 2;
        routes.targets=new address[](count);
        routes.callDatas=new bytes[](count);
        routes.amounts=new uint256[](count);
        if(o.firstAmount!=0) leg(routes,0,first,o.firstTier,o.firstAmount,o);
        if(net!=o.firstAmount) leg(routes,count-1,second,o.secondTier,net-o.firstAmount,o);
        EpsRouter.Permit memory empty;
        EpsRouter(router).swap(o.input,o.output,o.amount,o.minimum,o.minimum,false,routes,empty,"");
        transfer(o.input,abi.encodeCall(EpsToken.approve,(router,0)));
        if (balance(o.input,address(this))!=selfIn || balance(o.input,router)!=venueIn ||
            balance(o.output,router)!=venueOut || EpsToken(o.input).allowance(address(this),router)!=0) revert Payment();
        uint256 own=balance(o.output,address(this))-selfOut;
        transfer(o.output,abi.encodeCall(EpsToken.transfer,(msg.sender,own)));
        received=balance(o.output,msg.sender)-payerOut;
        if(payerIn-balance(o.input,msg.sender)!=o.amount || balance(o.output,address(this))!=selfOut ||
            received<o.minimum) revert Payment();
        emit Settled(msg.sender,o.input,o.output,keccak256(abi.encode(o)),o.amount,received,fee);
        entered=false;
    }
    function leg(EpsRouter.Routes memory r,uint256 i,address provider,uint24 tier,uint256 amount,Order calldata o) private view {
        bytes[] memory calls=new bytes[](1);
        calls[0]=abi.encodeCall(EpsProvider.exactInputSingle,(EpsProvider.Single(
            o.input,o.output,tier,router,amount,0,0)));
        r.targets[i]=provider;
        r.callDatas[i]=abi.encodeCall(EpsProvider.multicall,(o.deadline,calls));
        r.amounts[i]=amount;
    }
    function balance(address token,address holder) private view returns(uint256) {
        return EpsToken(token).balanceOf(holder);
    }
    function transfer(address token,bytes memory payload) private {
        (bool ok,bytes memory result)=token.call(payload);
        if(!ok || (result.length!=0 && !abi.decode(result,(bool)))) revert Payment();
    }
}

