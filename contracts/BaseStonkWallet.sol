// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Прямой обмен через опубликованный BaseStonk SimpleSwapRouter. Кошелёк задаёт срок,
// минимальную чистую выплату и точный расход. Хеш Router и связь с PoolManager
// закрепляются при размещении. Источник https://docs.basestonk.io/integration/contracts.md
interface IBaseStonkToken {
    function balanceOf(address) external view returns (uint256);
    function allowance(address,address) external view returns (uint256);
    function transferFrom(address,address,uint256) external returns (bool);
    function approve(address,uint256) external returns (bool);
    function transfer(address,uint256) external returns (bool);
}

interface IBaseStonkRouter {
    struct Key { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
    function poolManager() external view returns (address);
    function swap(Key calldata,bool,uint256,uint256,address) external returns (uint256);
}

contract BaseStonkWallet {
    IBaseStonkRouter public immutable router;
    bytes32 public immutable routerHash;
    address public immutable manager;
    bool transient private entered;
    error Invalid();
    event Settled(address indexed payer, bytes32 indexed poolId,address indexed input,address output,uint256 spent,uint256 received);

    constructor(address router_,bytes32 codeHash,address manager_) {
        if (router_.code.length==0 || router_.codehash!=codeHash || manager_.code.length==0 ||
            IBaseStonkRouter(router_).poolManager()!=manager_) revert Invalid();
        router=IBaseStonkRouter(router_);
        routerHash=codeHash;
        manager=manager_;
    }

    function execute(IBaseStonkRouter.Key calldata key,bool zeroForOne,uint256 amount,uint256 minimumDebit,uint256 minimum,uint256 deadline)
        external returns (uint256 received)
    {
        if (entered || address(router).codehash!=routerHash || deadline<block.timestamp || minimum==0 ||
            minimumDebit==0 || minimumDebit>amount || amount>uint256(uint128(type(int128).max)) || key.currency0==address(0) ||
            key.currency0>=key.currency1 || key.hooks.code.length==0) revert Invalid();
        entered=true;
        IBaseStonkToken input=IBaseStonkToken(zeroForOne?key.currency0:key.currency1);
        IBaseStonkToken output=IBaseStonkToken(zeroForOne?key.currency1:key.currency0);
        uint256 held=input.balanceOf(address(this));
        uint256 payerBalance=input.balanceOf(msg.sender);
        uint256 beforeOutput=output.balanceOf(msg.sender);
        if (input.allowance(address(this),address(router))!=0) revert Invalid();
        send(input,abi.encodeCall(input.transferFrom,(msg.sender,address(this),amount)));
        if(input.balanceOf(address(this))!=held+amount) revert Invalid();
        send(input,abi.encodeCall(input.approve,(address(router),amount)));
        router.swap(key,zeroForOne,amount,minimum,msg.sender);
        uint256 change=input.balanceOf(address(this))-held;
        uint256 spent=amount-change;
        if(spent<minimumDebit) revert Invalid();
        if(change!=0) send(input,abi.encodeCall(input.transfer,(msg.sender,change)));
        received=output.balanceOf(msg.sender)-beforeOutput;
        if(received<minimum || input.balanceOf(msg.sender)+spent!=payerBalance ||
            input.balanceOf(address(this))!=held) revert Invalid();
        if(input.allowance(address(this),address(router))!=0) {
            send(input,abi.encodeCall(input.approve,(address(router),0)));
            if(input.allowance(address(this),address(router))!=0) revert Invalid();
        }
        emit Settled(msg.sender,keccak256(abi.encode(key)),address(input),address(output),spent,received);
        entered=false;
    }

    function send(IBaseStonkToken asset,bytes memory data) private {
        (bool success,bytes memory answer)=address(asset).call(data);
        if(!success || (answer.length!=0 && !abi.decode(answer,(bool)))) revert Invalid();
    }
}

