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
//   ICLPoolActions.sol L131-L137) и обратные вызовы uniswapV3SwapCallback / pancakeV3SwapCallback / algebraSwapCallback.
// Настройки: нет. Пределы цены — TickMath.MIN_SQRT_RATIO + 1 / MAX_SQRT_RATIO - 1 (те же числа в v3-core TickMath.sol L14, L16).
pragma solidity 0.8.37;

interface IV3LikePool {
    function swap(address recipient, bool zeroForOne, int256 amountSpecified, uint160 sqrtPriceLimitX96, bytes calldata data)
        external
        returns (int256 amount0, int256 amount1);
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
}
