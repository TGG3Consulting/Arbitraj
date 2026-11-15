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

// iZiSwap / Biswap V3 (решение 286): swapX2Y / swapY2X(address,uint128,int24,bytes) + swapX2YCallback(x, y, data) /
// swapY2XCallback(x, y, data) — izumiFinance/iZiSwap-core@3fe21eba IiZiSwapPool.sol, IiZiSwapCallback.sol L19-L39;
// BiswapPoolV3 (Sourcify exact_match 56/0x76008e…3094) — те же селекторы и вызовы. Пул пробрасывает откат модуля как
// есть ТОЛЬКО длиной 64 байта (iZiSwapPool.sol L175-L192 revertDCData, BiswapPoolV3.sol L169-L186), иначе — «dc» /
// строка. state() — второе слово currentPoint (libraries/State.sol у обоих), leftMostPt / rightMostPt — геттеры пула.
interface IIziPoolQ {
    function swapX2Y(address recipient, uint128 amount, int24 lowPt, bytes calldata data) external;
    function swapY2X(address recipient, uint128 amount, int24 highPt, bytes calldata data) external;
    function state() external view returns (uint160 sqrtPrice_96, int24 currentPoint);
    function leftMostPt() external view returns (int24);
    function rightMostPt() external view returns (int24);
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

    // Решение 286: выход пула iZiSwap / Biswap V3 на точный вход. x2y — продажа tokenX (меньший адрес пары). Пул может
    // взять чуть меньше суммы (округление точек — остаток у вызывающего), поэтому «взял весь вход» решает не сравнение
    // сумм, а край пула: обмен дошёл до leftMostPt (x2y) / rightMostPt (y2x) — ликвидность кончилась, used = сколько пул
    // взял на самом деле (бот видит used < amountIn — «не хватает ликвидности»); иначе used = amountIn. out — выход пула.
    function quoteIzi(address pool, bool x2y, uint256 amountIn) external returns (uint256 used, uint256 out) {
        require(amountIn > 0 && amountIn <= type(uint128).max, "amount");
        bytes memory r;
        if (x2y) {
            try IIziPoolQ(pool).swapX2Y(address(this), uint128(amountIn), type(int24).min, "") {
                revert("VQ: no callback");
            } catch (bytes memory e) {
                r = e;
            }
        } else {
            try IIziPoolQ(pool).swapY2X(address(this), uint128(amountIn), type(int24).max, "") {
                revert("VQ: no callback");
            } catch (bytes memory e) {
                r = e;
            }
        }
        if (r.length != 64) {
            assembly {
                revert(add(r, 32), mload(r))
            }
        }
        (uint256 w0, uint256 w1) = abi.decode(r, (uint256, uint256));
        bool edge = w0 >> 255 == 1;
        used = edge ? w0 & ((1 << 255) - 1) : amountIn;
        out = w1;
    }

    // Обратные вызовы iZiSwap: откат ровно 64 байтами — (оплата | край << 255, выход). Край — точка пула после обмена
    // (state() записан до обратного вызова: swapX2Y.sol L319-L328) на границе leftMostPt / rightMostPt.
    function swapX2YCallback(uint256 x, uint256 y, bytes calldata) external view {
        (, int24 pt) = IIziPoolQ(msg.sender).state();
        _izi(x, y, pt <= IIziPoolQ(msg.sender).leftMostPt());
    }

    function swapY2XCallback(uint256 x, uint256 y, bytes calldata) external view {
        (, int24 pt) = IIziPoolQ(msg.sender).state();
        _izi(y, x, pt >= IIziPoolQ(msg.sender).rightMostPt());
    }

    function _izi(uint256 paid, uint256 got, bool edge) internal pure {
        uint256 w0 = paid | (edge ? 1 << 255 : 0);
        assembly {
            mstore(0, w0)
            mstore(32, got)
            revert(0, 64)
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
