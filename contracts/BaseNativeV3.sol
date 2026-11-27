// SPDX-License-Identifier: MIT
// Файл: contracts/BaseNativeV3.sol.
// Что делает: сверяет пул и фабрику форков V3 на Base и вызывает подходящий роутер с минимумом и сроком.
// Кто вызывает: BasePmmExecutor.execute при определении и оплате маршрута через отдельный V3-пул.
// Зависит: getPool фабрики, текущая fee пула или swapFeeUnits у Horizon и соответствующий ABI роутера.
pragma solidity 0.8.37;

interface IBaseForkPool {
    function fee() external view returns (uint24);
    function factory() external view returns (address);
}

interface IBaseElastic {
    function swapFeeUnits() external view returns(uint24);
    function swapExactInputSingle(IBaseForkRouter.Input calldata order) external payable returns(uint256);
}

interface IBaseForkFactory {
    function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address);
}

interface IBaseForkRouter {
    struct Input {
        address tokenIn;
        address tokenOut;
        uint24 fee;
        address recipient;
        uint256 deadline;
        uint256 amountIn;
        uint256 amountOutMinimum;
        uint160 sqrtPriceLimitX96;
    }

    function exactInputSingle(Input calldata input) external payable returns (uint256 amountOut);
}

/// The three verified Base deployments use the same order ABI but distinct pool callbacks.
/// Resolve the pool against its actual factory before approving the corresponding native router.
library BaseNativeV3 {
    address private constant HORIZON=address(bytes20(hex"99aec509174cbf06f8f7e15ddeeb7bcc32363827"));

    function market(address pool, address a, address b) internal view returns (address router, uint24 fee) {
        if (a == b || a == address(0) || b == address(0)) return (address(0), 0);
        (bool ok, bytes memory value) = pool.staticcall(abi.encodeCall(IBaseForkPool.factory, ()));
        if (!ok || value.length != 32) return (address(0), 0);
        address factory = abi.decode(value, (address));
        if (factory == address(bytes20(hex"7b72c4002ea7c276dd717b96b20f4956c5c904e7"))) {
            router = address(bytes20(hex"a07d063b595168e081b51280ada5fc8e11cde52b"));
        } else if (factory == address(bytes20(hex"e8839bf8175812691c6578c0fc80e721bc3e00fb"))) {
            router = address(bytes20(hex"48e285bc3690415c3b6220c3f833ed9c303a4f46"));
        } else if (factory == address(bytes20(hex"d6ab0566e7e60b67c50ac73ddff4e3ddcb829ec2"))) {
            router = address(bytes20(hex"34c97b800876d9ca4658561f00512fa7abdaf315"));
        } else if (factory==address(bytes20(hex"07aced5690e09935b1c0e6e88b772d9440f64718"))) {
            router=HORIZON;
        } else {
            return (address(0), 0);
        }
        fee = router==HORIZON ? IBaseElastic(pool).swapFeeUnits() : IBaseForkPool(pool).fee();
        if (IBaseForkFactory(factory).getPool(a, b, fee) != pool) return (address(0), 0);
    }

    function swap(address router, uint24 fee, address a, address b, uint256 amount, uint256 minimum,
                  uint256 deadline, address recipient) internal {
        IBaseForkRouter.Input memory order=IBaseForkRouter.Input(a,b,fee,recipient,deadline,amount,minimum,0);
        if(router==HORIZON) IBaseElastic(router).swapExactInputSingle(order);
        else IBaseForkRouter(router).exactInputSingle(order);
    }
}
