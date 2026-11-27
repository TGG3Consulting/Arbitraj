// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Обмен собственного ERC20 по публичной котировке OpenOcean, напрямую или через Epsilon.
// Проверяет источник и получателя оплаты внутри официального ABI, срок и чистый остаток кошелька.
interface EAAsset {
    function balanceOf(address) external view returns(uint256);
    function allowance(address,address) external view returns(uint256);
    function transferFrom(address,address,uint256) external returns(bool);
    function approve(address,uint256) external returns(bool);
    function transfer(address,uint256) external returns(bool);
}
interface EAVenue {
    struct Routes{address[] targets; bytes[] callDatas; uint256[] amounts;}
    struct Permission{address token;uint256 amount;}
    struct Permit{Permission permitted;uint256 nonce;uint256 deadline;}
    function protocolFeeBps() external view returns(uint16);
    function swap(address,address,uint256,uint256,uint256,bool,Routes calldata,Permit calldata,bytes calldata) external payable returns(uint256);
}
interface EAOcean {
    struct Description{address src;address dst;address from;address receiver;uint256 amount;uint256 minimum;uint256 guaranteed;uint256 flags;address referrer;bytes permit;}
    struct Call{uint256 target;uint256 gasLimit;uint256 value;bytes data;}
    function swap(address executor,Description calldata trade,Call[] calldata steps) external payable returns(uint256);
}
contract EpsilonApiWallet {
    address public immutable router;
    address public immutable ocean;
    address public immutable executor;
    address public immutable fundingReceiver;
    bytes32 public immutable fundingHash;
    bytes32 public immutable routerHash;
    bytes32 public immutable oceanHash;
    bytes32 public immutable executorHash;
    bool transient private busy;
    error Route();
    error Funds();
    constructor(address e,address o,address x,address receiver){
        if(e.code.length==0||o.code.length==0||x.code.length==0||e==o||e==x||o==x)revert Route();
        if (receiver!=address(0)&&(receiver.code.length==0||receiver==e||receiver==o||receiver==x))revert Route();
        fundingReceiver=receiver;fundingHash=receiver.codehash;
        router=e;ocean=o;executor=x;routerHash=e.codehash;oceanHash=o.codehash;executorHash=x.codehash;
    }
    struct Order{address input;address output;uint256 amount;uint256 minimum;uint256 deadline;uint16 feeBps;bool throughVenue;}
    struct Balances{uint256 mineIn;uint256 mineOut;uint256 venueIn;uint256 venueOut;uint256 payerIn;uint256 payerOut;}
    event Settled(address indexed payer,address indexed input,address indexed output,bytes32 orderHash,
        uint256 spent,uint256 received,uint256 protocolFee,bool throughVenue);
    function trade(Order calldata o,bytes calldata payload) external returns(uint256 received){
        if(busy||block.timestamp>o.deadline||o.input==o.output||o.minimum==0||o.amount==0||o.feeBps>=10000||
            router.codehash!=routerHash||ocean.codehash!=oceanHash||executor.codehash!=executorHash)revert Route();
        busy=true;
        uint256 fee;
        if(o.throughVenue){
            if(EAVenue(router).protocolFeeBps()!=o.feeBps)revert Route();
            fee=o.amount*uint256(o.feeBps)/10000;
        }else if(o.feeBps!=0)revert Route();
        address target=o.throughVenue?router:ocean;
        address receiver=o.throughVenue?router:address(this);
        bool prepaid=checkPayload(o,payload,receiver,o.amount-fee);
        if(prepaid&&EAAsset(o.input).balanceOf(fundingReceiver)!=0)revert Funds();
        Balances memory b=balances(o);
        if(EAAsset(o.input).allowance(address(this),target)!=0)revert Funds();
        token(o.input,abi.encodeCall(EAAsset.transferFrom,(msg.sender,address(this),o.amount)));
        if(EAAsset(o.input).balanceOf(address(this))!=b.mineIn+o.amount)revert Funds();
        token(o.input,abi.encodeCall(EAAsset.approve,(target,o.amount)));
        if(o.throughVenue){
            EAVenue.Routes memory r;r.targets=new address[](1);r.targets[0]=ocean;r.amounts=new uint256[](1);
            r.amounts[0]=o.amount-fee;r.callDatas=new bytes[](1);r.callDatas[0]=payload;
            EAVenue.Permit memory empty;
            EAVenue(router).swap(o.input,o.output,o.amount,o.minimum,o.minimum,false,r,empty,"");
        }else{
            (bool ok,bytes memory ret)=ocean.call(payload);if(!ok||ret.length!=32)revert Funds();
        }
        token(o.input,abi.encodeCall(EAAsset.approve,(target,0)));
        if(prepaid&&EAAsset(o.input).balanceOf(fundingReceiver)!=0)revert Funds();
        uint256 gain=EAAsset(o.output).balanceOf(address(this))-b.mineOut;
        if(gain<o.minimum)revert Funds();
        token(o.output,abi.encodeCall(EAAsset.transfer,(msg.sender,gain)));
        Balances memory a=balances(o);
        if(a.mineIn!=b.mineIn||a.mineOut!=b.mineOut||a.venueIn!=b.venueIn||a.venueOut!=b.venueOut||
            b.payerIn-a.payerIn!=o.amount||EAAsset(o.input).allowance(address(this),target)!=0)revert Funds();
        received=a.payerOut-b.payerOut;if(received<o.minimum)revert Funds();
        emit Settled(msg.sender,o.input,o.output,keccak256(abi.encode(o,keccak256(payload))),o.amount,received,fee,o.throughVenue);
        busy=false;
    }
    function checkPayload(Order calldata o,bytes calldata p,address receiver,uint256 amount) private view returns(bool prepaid) {
        if(bytes4(p[:4])!=EAOcean.swap.selector)revert Route();
        (address who,EAOcean.Description memory d,EAOcean.Call[] memory calls)=abi.decode(p[4:],(address,EAOcean.Description,EAOcean.Call[]));
        if(who!=executor||d.receiver!=receiver||d.src!=o.input||d.dst!=o.output||d.amount!=amount||
            d.minimum==0||d.flags!=2||d.permit.length!=0||calls.length==0||d.referrer!=address(0)||
            keccak256(p)!=keccak256(abi.encodeCall(EAOcean.swap,(who,d,calls))))revert Route();
        if(d.from==executor)return false;
        if(d.from==address(0)||d.from!=fundingReceiver||d.from.codehash!=fundingHash)revert Route();
        EAOcean.Call memory payment=calls[0];
        if(payment.target!=uint256(uint160(d.from))||payment.value!=0)revert Route();
        // Штатное исполнение принимает только собственную предоплату и отдаёт этот выход исполнителю OpenOcean.
        bytes memory operation=payment.data;
        if (operation.length<4 || bytes4(operation)!=bytes4(keccak256("swapWithContractBalance(address,address,uint256,address,bytes)")))revert Route();
        bytes memory args=new bytes(operation.length-4);
        assembly ("memory-safe"){mcopy(add(args,32),add(operation,36),mload(args))}
        (address a,address b,uint256 bound,address payee,bytes memory terms)=abi.decode(args,(address,address,uint256,address,bytes));
        if(a!=o.input||b!=o.output||payee!=executor||keccak256(operation)!=keccak256(abi.encodeWithSelector(bytes4(operation),a,b,bound,payee,terms)))revert Route();
        return true;
    }
    function balances(Order calldata o) private view returns(Balances memory){
        EAAsset a=EAAsset(o.input);EAAsset b=EAAsset(o.output);
        return Balances(a.balanceOf(address(this)),b.balanceOf(address(this)),a.balanceOf(router),b.balanceOf(router),
            a.balanceOf(msg.sender),b.balanceOf(msg.sender));
    }
    function token(address to,bytes memory data) private {
        (bool ok,bytes memory r)=to.call(data);if(!ok||(r.length!=0&&!abi.decode(r,(bool))))revert Funds();
    }
}
