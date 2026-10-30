# venues/bsc_dex/math — точная математика пулов

Порты официальных библиотек контрактов на целых числах (U256, промежуточные — U512), с теми же округлениями.
Где контракт откатился бы — `None`. Тесты — официальные тестовые значения протоколов (`tests/bsc_dex_math.rs`, тесты в файлах).

| Файл | Что делает | Источник |
| --- | --- | --- |
| `full_math.rs` | mulDiv (вниз/вверх), divRoundingUp, сдвиги 128.128 | FullMath, UnsafeMath, Uint256x256Math |
| `tick_math.rs` | getSqrtRatioAtTick | TickMath (тест — v3-core TickMath.spec.ts) |
| `sqrt_price_math.rs` | Следующая цена, суммы между ценами | SqrtPriceMath |
| `swap_step.rs` | Шаг обмена V3 и V4 (разный знак и формула комиссии) | SwapMath V3 / V4 (тесты — v3-core, v4-core) |
| `swap_cl.rs` | Цикл обмена V3/CL по карте тиков, точный вход и выход; swap_to — до предела цены (sqrtPriceLimitX96; глубина в полосе — решение 73) | PancakeV3Pool.swap, v4 Pool.swap |
| `swap_elastic_step.rs` | Шаг-математика KyberSwap Elastic (решение 81): computeSwapStep + calcReachAmount / estimate+calcIncrementalLiquidity / calcFinalPrice / calcReturnedAmount + QuadMath (меньший корень). Reinvestment liquidity: L=baseL+reinvestL, комиссия шага → ΔL; свои формулы (BPS/TWO_FEE_UNITS) | ks-elastic-sc SwapMath.sol, QuadMath.sol (тесты — логи SwapMath.spec.ts) |
| `swap_elastic.rs` | Цикл обмена Elastic по карте тиков (Pool.swap): шаги swap_elastic_step, MAX_TICK_DISTANCE=480, reinvestL переживает тики, пересечение baseL±=liquidityNet; swap_to — до предела цены. TickMath=V3 (tick_math.rs); свой поиск тика (не word-границы — ΔL компаундинг); переэкспорт шага для котировки | ks-elastic-sc Pool.sol, PoolTicksState.sol |
| `v2.rs` | getAmountOut / getAmountIn с комиссией из настройки | PancakeLibrary, UniswapV2Library (тест — v2-periphery) |
| `bin_price.rs` | Цена бина 128.128 | PriceHelper, Uint128x128Math.pow (тест — PriceHelper.t.sol) |
| `bin_swap.rs` | Обмен в пуле Bin по резервам бинов | BinPool.swap, BinHelper, FeeHelper |
