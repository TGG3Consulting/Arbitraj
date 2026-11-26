// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

interface IBaseForkPool {
    function fee() external view returns (uint24);
    function factory() external view returns (address);
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
        } else {
            return (address(0), 0);
        }
        fee = IBaseForkPool(pool).fee();
        if (IBaseForkFactory(factory).getPool(a, b, fee) != pool) return (address(0), 0);
    }

    function swap(address router, uint24 fee, address a, address b, uint256 amount, uint256 minimum,
                  uint256 deadline, address recipient) internal {
        IBaseForkRouter(router).exactInputSingle(IBaseForkRouter.Input(a,b,fee,recipient,deadline,amount,minimum,0));
    }
}
