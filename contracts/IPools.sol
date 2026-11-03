// SPDX-License-Identifier: MIT
// Файл: contracts/IPools.sol
// Что делает: интерфейсы токена и пулов DEX для исполнителя ArbExecutor.sol — только функции, которые он вызывает,
//   с подписями из официальных исходников (репозиторий@коммит, файл, строки — у каждого интерфейса).
// Кто вызывает: contracts/ArbExecutor.sol (import).
// От чего зависит: ничего.
// Настройки: нет.
pragma solidity 0.8.37;

// ERC-20 — https://eips.ethereum.org/EIPS/eip-20
interface IERC20 {
    function balanceOf(address who) external view returns (uint256);
    function transfer(address to, uint256 value) external returns (bool);
    function transferFrom(address from, address to, uint256 value) external returns (bool);
    function approve(address spender, uint256 value) external returns (bool);
}

// Uniswap V2 / PancakeSwap V2 и форки: Uniswap/v2-core@6a9e7c97 contracts/UniswapV2Pair.sol L38, L159;
// pancakeswap/pancake-swap-core@3b214306 contracts/PancakePair.sol L38, L159.
interface IV2Pair {
    function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata data) external;
}

// Solidly (THENA V1, Velodrome V1): ThenafiBNB/THENA-Contracts@f5bc742e contracts/Pair.sol L239, L379, L459;
// velodrome-finance/v1 contracts/Pair.sol L198, L338, L416.
interface ISolidlyPair {
    function getReserves() external view returns (uint256 reserve0, uint256 reserve1, uint256 blockTimestampLast);
    function getAmountOut(uint256 amountIn, address tokenIn) external view returns (uint256);
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata data) external;
}

// Uniswap V3 / PancakeSwap V3 / Slipstream (Topaz) / Algebra V1.9 (THENA Fusion) — одна и та же подпись swap:
// Uniswap/v3-core@5ea68a77 contracts/interfaces/pool/IUniswapV3PoolActions.sol L75-L81;
// pancakeswap/pancake-v3-contracts@98684794 projects/v3-core/contracts/interfaces/pool/IPancakeV3PoolActions.sol L75-L81;
// velodrome-finance/slipstream@193a6b85 contracts/core/interfaces/pool/ICLPoolActions.sol L131-L137;
// npm @cryptoalgebra/v1.9-core@1.0.0 contracts/interfaces/pool/IAlgebraPoolActions.sol L96-L102.
interface IV3Pool {
    function swap(address recipient, bool zeroForOne, int256 amountSpecified, uint160 sqrtPriceLimitX96, bytes calldata data)
        external returns (int256 amount0, int256 amount1);
}

// KyberSwap Elastic: KyberNetwork/ks-elastic-sc@4ab08c0a contracts/interfaces/pool/IPoolActions.sol L88-L94.
interface IElasticPool {
    function swap(address recipient, int256 swapQty, bool isToken0, uint160 limitSqrtP, bytes calldata data)
        external returns (int256 qty0, int256 qty1);
}

// Curve / Ellipsis (плоский StableSwap, индексы int128): curvefi/curve-contract@574f4402
// contracts/pool-templates/base/SwapTemplateBase.vy L447; ellipsis-finance/ellipsis@6bdf7788 contracts/StableSwap.vy L436.
// Возвращаемое значение не читается (у Ellipsis его нет) — выход меряется по балансу.
interface ICurvePool {
    function exchange(int128 i, int128 j, uint256 dx, uint256 minDy) external;
}

// DODO V2 (DVM/DPP/DSP): DODOEX/contractV2@2f1bcdac contracts/DODOVendingMachine/impl/DVMTrader.sol L41, L65,
// DVMStorage.sol L30 (_BASE_TOKEN_); DPPTrader.sol, DSPTrader.sol — те же sellBase/sellQuote.
interface IDodoPool {
    function _BASE_TOKEN_() external view returns (address);
    function sellBase(address to) external returns (uint256);
    function sellQuote(address to) external returns (uint256);
}

// WooFi WooPPV2: woonetwork/WooPoolV2@f73a0f64 contracts/WooPPV2.sol L152-L170 (вход заранее в пул, L443).
interface IWooPP {
    function swap(address fromToken, address toToken, uint256 fromAmount, uint256 minToAmount, address to, address rebateTo)
        external returns (uint256 realToAmount);
}

// Maverick V2: maverickprotocol/v2-common@d91e8714 contracts/interfaces/IMaverickV2Pool.sol L106-L111 (SwapParams),
// L233 (tokenA), L361-L386 (swap: обратный вызов только при непустых data).
interface IMaverickV2Pool {
    struct SwapParams {
        uint256 amount;
        bool tokenAIn;
        bool exactOutput;
        int32 tickLimit;
    }
    function tokenA() external view returns (address);
    function swap(address recipient, SwapParams memory params, bytes calldata data)
        external returns (uint256 amountIn, uint256 amountOut);
}
