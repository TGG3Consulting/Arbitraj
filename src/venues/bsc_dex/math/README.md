# venues/bsc_dex/math — точная математика пулов

Порты официальных библиотек контрактов на целых числах (U256, промежуточные — U512), с теми же округлениями.
Где контракт откатился бы — `None`. Тесты — официальные тестовые значения протоколов (`tests/bsc_dex_math.rs`, тесты в файлах).

| Файл | Что делает | Источник |
| --- | --- | --- |
| `full_math.rs` | mulDiv (вниз/вверх), divRoundingUp, сдвиги 128.128 | FullMath, UnsafeMath, Uint256x256Math |
| `tick_math.rs` | getSqrtRatioAtTick | TickMath (тест — v3-core TickMath.spec.ts) |
| `sqrt_price_math.rs` | Следующая цена, суммы между ценами | SqrtPriceMath |
| `swap_step.rs` | Шаг обмена V3 и V4 (разный знак и формула комиссии) | SwapMath V3 / V4 (тесты — v3-core, v4-core) |
| `swap_cl.rs` | Цикл обмена V3/CL по карте тиков, точный вход и выход | PancakeV3Pool.swap, v4 Pool.swap |
| `v2.rs` | getAmountOut / getAmountIn с комиссией из настройки | PancakeLibrary, UniswapV2Library (тест — v2-periphery) |
| `bin_price.rs` | Цена бина 128.128 | PriceHelper, Uint128x128Math.pow (тест — PriceHelper.t.sol) |
| `bin_swap.rs` | Обмен в пуле Bin по резервам бинов | BinPool.swap, BinHelper, FeeHelper |
