// SPDX-License-Identifier: UNLICENSED
// Файл: contracts/VirtualQuoter.sol
// Что делает: «виртуальный квотер» (решение 186, Тигран 05.11.2026: «нам нужны все DEX»). Никогда не разворачивается в
//   сети: бот подставляет его код по произвольному адресу в eth_call (state override, переопределение кода счёта) и
//   спрашивает выход обмена у САМОГО пула — пул исполняет свой настоящий swap, в обратном вызове квотер откатывает всё,
//   отдавая дельты пула. Так считают и официальные QuoterV2 (Uniswap v3-periphery QuoterV2.sol: «reverts in the
//   callback»), но без поиска пула по фабрике — нужен только адрес пула. Деньги не трогаются: вызов — eth_call.
// Кто вызывает: бот (src/venues/bsc_dex/quote/virtual_quoter.rs) через eth_call с переопределением кода.
// От чего зависит: интерфейсы swap пулов V3 / форков V3 / Algebra / Slipstream — та же подпись
//   swap(address,bool,int256,uint160,bytes) (Uniswap/v3-core IUniswapV3PoolActions.sol L75-L81; pancake-v3-contracts
//   IPancakeV3PoolActions.sol L75-L81; @cryptoalgebra/v1.9-core IAlgebraPoolActions.sol L96-L102; slipstream
//   ICLPoolActions.sol L131-L137) и обратные вызовы uniswapV3SwapCallback / pancakeV3SwapCallback / algebraSwapCallback,
//   а также swapCallback пула SheepDex (решение 274: SpePool — форк Uniswap V3 с тем же swap, обратный вызов
//   ISpeSwapCallback.swapCallback(int256,int256,bytes); verified-исходник Sourcify partial_match 56/0x1978…5f62,
//   SpePool.sol L1757-L1762, вызов в swap L2447, L2453), и squadV3SwapCallback пула SquadSwap V3 (решение 281: SquadV3Pool —
//   форк PancakeSwap V3 с тем же swap, обратный вызов ISquadV3SwapCallback.squadV3SwapCallback(int256,int256,bytes),
//   селектор 0xc4e21d86; verified-исходник Sourcify partial_match 56/0x9fb9815ca1417b06466aaa86521a28ba83d0bd33,
//   SquadV3Pool.sol (sha256 88dd3da0…9283) — swap L603, вызов L794, L800).
// Настройки: нет. Пределы цены — TickMath.MIN_SQRT_RATIO + 1 / MAX_SQRT_RATIO - 1 (те же числа в v3-core TickMath.sol L14, L16).
pragma solidity 0.8.37;

interface IV3LikePool {
    function swap(address recipient, bool zeroForOne, int256 amountSpecified, uint160 sqrtPriceLimitX96, bytes calldata data)
        external
        returns (int256 amount0, int256 amount1);
}

// Metric OMM (решение 285): swap(address,bool,int128,uint128,bytes,bytes) + metricOmmSwapCallback(int256,int256,bytes) —
// ABI SDK @nradko/metric-omm-sdk-v1@0.4.5 (src/abis/MetricOmmPool.ts, MetricOmmSimpleRouter.ts), селекторы есть в коде
// пулов BSC (tests/data/metric_2026-11-15). Пределы цены 0 / uint128.max — как проба на цепи.
interface IMetricOmmPoolQ {
    function swap(address recipient, bool zeroForOne, int128 amountSpecified, uint128 priceLimitX64, bytes calldata callbackData, bytes calldata extensionData)
        external
        returns (int128, int128);
}

contract VirtualQuoter {
    uint160 internal constant MIN_SQRT_RATIO_PLUS_1 = 4295128740;
    uint160 internal constant MAX_SQRT_RATIO_MINUS_1 = 1461446703485210103287273052203988822378723970341;

    /// Точный вход: сколько пул отдаст за amountIn. Возвращает (сколько входа пул взял бы, сколько отдаст).
    /// Пул взял меньше amountIn — ликвидности до предела цены не хватило (бот это видит и не считает квоту полной).
    /// Откат пула по другой причине — его текст пробрасывается как есть.
    function quoteExactIn(address pool, bool zeroForOne, uint256 amountIn) external returns (uint256 used, uint256 out) {
        require(amountIn <= uint256(type(int256).max), "amount");
        uint160 limit = zeroForOne ? MIN_SQRT_RATIO_PLUS_1 : MAX_SQRT_RATIO_MINUS_1;
        try IV3LikePool(pool).swap(address(this), zeroForOne, int256(amountIn), limit, "") {
            revert("VQ: no callback");
        } catch (bytes memory r) {
            // Свой откат из обратного вызова — ровно 64 байта (две дельты). Всё остальное — откат пула.
            if (r.length != 64) {
                assembly {
                    revert(add(r, 32), mload(r))
                }
            }
            (int256 d0, int256 d1) = abi.decode(r, (int256, int256));
            int256 paid = zeroForOne ? d0 : d1;
            int256 got = zeroForOne ? d1 : d0;
            used = paid > 0 ? uint256(paid) : 0;
            out = got < 0 ? uint256(-got) : 0;
        }
    }

    // Решение 285: выход пула Metric OMM на точный вход (как quoteExactIn): swap → откат в metricOmmSwapCallback с
    // дельтами. Возвращает: (взял входа, отдаст).
    function quoteMetric(address pool, bool zeroForOne, uint256 amountIn) external returns (uint256 used, uint256 out) {
        require(amountIn <= uint256(uint128(type(int128).max)), "amount");
        try IMetricOmmPoolQ(pool).swap(
            address(this), zeroForOne, int128(int256(amountIn)), zeroForOne ? 0 : type(uint128).max, hex"01", ""
        ) {
            revert("VQ: no callback");
        } catch (bytes memory r) {
            if (r.length != 64) {
                assembly {
                    revert(add(r, 32), mload(r))
                }
            }
            (int256 d0, int256 d1) = abi.decode(r, (int256, int256));
            int256 paid = zeroForOne ? d0 : d1;
            int256 got = zeroForOne ? d1 : d0;
            used = paid > 0 ? uint256(paid) : 0;
            out = got < 0 ? uint256(-got) : 0;
        }
    }

    function _back(int256 d0, int256 d1) internal pure {
        assembly {
            mstore(0, d0)
            mstore(32, d1)
            revert(0, 64)
        }
    }

    function uniswapV3SwapCallback(int256 d0, int256 d1, bytes calldata) external pure { _back(d0, d1); }
    function pancakeV3SwapCallback(int256 d0, int256 d1, bytes calldata) external pure { _back(d0, d1); }
    function algebraSwapCallback(int256 d0, int256 d1, bytes calldata) external pure { _back(d0, d1); }
    function swapCallback(int256 d0, int256 d1, bytes calldata) external pure { _back(d0, d1); }
    function squadV3SwapCallback(int256 d0, int256 d1, bytes calldata) external pure { _back(d0, d1); }
    function metricOmmSwapCallback(int256 d0, int256 d1, bytes calldata) external pure { _back(d0, d1); }
}
