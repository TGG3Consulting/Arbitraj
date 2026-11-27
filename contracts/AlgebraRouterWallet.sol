// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Покупка и продажа через штатный Algebra Router. Плательщик задаёт предел расхода и
// минимум на своём кошельке. Подтверждение результата учитывает округление токенов с долями.
interface IAlgebraWalletToken {
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function approve(address spender, uint256 amount) external returns (bool);
}
interface IAlgebraWalletFactory {
    function poolDeployer() external view returns (address);
    function poolByPair(address a, address b) external view returns (address);
    function customPoolByPair(address deployer, address a, address b) external view returns (address);
}
interface IAlgebraWalletRouter {
    struct Order {address tokenIn;address tokenOut;address deployer;address recipient;uint256 deadline;uint256 amountIn;uint256 minimum;uint160 limit;}
    function exactInputSingle(Order calldata params) external payable returns (uint256);
    function factory() external view returns (address);
    function poolDeployer() external view returns (address);
}

contract AlgebraRouterWallet {
    IAlgebraWalletFactory public immutable factory;
    IAlgebraWalletRouter public immutable router;
    bool transient private busy;
    error InvalidOrder();
    error BadTransfer();
    error WalletLimit();
    event Settled(address indexed payer,address indexed pool,address indexed input,address output,uint256 spent,uint256 received);

    constructor(IAlgebraWalletFactory f, IAlgebraWalletRouter r) {
        if (address(f).code.length == 0 || address(r).code.length == 0 || r.factory()!=address(f)
            || f.poolDeployer().code.length==0 || f.poolDeployer()!=r.poolDeployer()) revert InvalidOrder();
        factory=f;
        router=r;
    }

    function execute(address pool,address input,address output,address deployer,uint256 amount,uint256 minimum,uint256 deadline)
        external returns (uint256 spent,uint256 received) {
        if (busy || pool.code.length==0 || input==output || amount==0 || minimum==0 || block.timestamp>deadline)
            revert InvalidOrder();
        if ((deployer==address(0) ? factory.poolByPair(input,output) : factory.customPoolByPair(deployer,input,output))!=pool)
            revert InvalidOrder();
        busy=true;
        IAlgebraWalletToken src=IAlgebraWalletToken(input);
        IAlgebraWalletToken dst=IAlgebraWalletToken(output);
        uint256 ownerIn=src.balanceOf(msg.sender);
        uint256 ownerOut=dst.balanceOf(msg.sender);
        uint256 prior=src.balanceOf(address(this));
        if (src.allowance(address(this),address(router))!=0) revert InvalidOrder();
        callToken(input,abi.encodeCall(src.transferFrom,(msg.sender,address(this),amount)));
        if (src.balanceOf(address(this))<=prior) revert BadTransfer();
        callToken(input,abi.encodeCall(src.approve,(address(router),amount)));
        router.exactInputSingle(IAlgebraWalletRouter.Order(input,output,deployer,msg.sender,deadline,amount,minimum,0));
        if (src.allowance(address(this),address(router))!=0)
            callToken(input,abi.encodeCall(src.approve,(address(router),0)));
        uint256 change=src.balanceOf(address(this))-prior;
        if (change!=0) callToken(input,abi.encodeCall(src.transfer,(msg.sender,change)));
        spent=ownerIn-src.balanceOf(msg.sender);
        received=dst.balanceOf(msg.sender)-ownerOut;
        if (spent==0 || spent>amount || received<minimum || src.balanceOf(address(this))!=prior
            || src.allowance(address(this),address(router))!=0) revert WalletLimit();
        emit Settled(msg.sender,pool,input,output,spent,received);
        busy=false;
    }

    function callToken(address token, bytes memory data) private {
        (bool ok,bytes memory value)=token.call(data);
        if (!ok || (value.length!=0 && (value.length!=32 || !abi.decode(value,(bool))))) revert BadTransfer();
    }
}
