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
| `dodo_math.rs` | Базовая математика кривой DODO PMM (решение 80): DecimalMath (фикс-точка 18 знаков) + DODOMath `_GeneralIntegrate` / `_SolveQuadraticFunctionForTarget` / `_SolveQuadraticFunctionForTrade` + SafeMath.sqrt/divCeil. Сырое `*` (переполнение mod 2^256, ручная проверка) и SafeMath `.mul` (откат) различаются дословно | DODOEX/contractV2 DecimalMath.sol, DODOMath.sol, SafeMath.sol |
| `swap_dodo.rs` | Кривая DODO PMM (V2: DVM/DPP/DSP): `sell_base_token` / `sell_quote_token` по R-состояниям (ONE/ABOVE_ONE/BELOW_ONE) с угловыми случаями возврата к R=1, `adjusted_target`, `get_mid_price`. Classical V1 (оракул Chainlink) не берётся | DODOEX/contractV2 PMMPricing.sol (тесты — on-chain assert'ы DVM/DSP trader.test.ts) |
| `maverick_math.rs` | Базовая фикс-точка Maverick V2 (решения 80, 73): `Math.sol` на U256 (D18, ONE=10^18) — min/max/clip/boundValue, mulFloor/mulCeil/divFloor/divCeil, mulDivDown/mulDivUp (одинарной ширины, откат при 2^256) против OzMath.mulDiv (512-бит), invFloor/invCeil, sqrt (solmate), abs/abs32, scale/ammScale, floorD8. `unchecked` → wrapping_* | maverickprotocol/v2-common `Math.sol` (тест — Math.t.sol: inv/clip/mulDiv/scale/abs/floor, точно) |
| `maverick_tick_math.rs` | Тиковая математика Maverick V2: `TickMath.sol` — tickSqrtPrice/tickSqrtPrices (sqrt-цена края тика, D18, 1.0001^tick, MAX_TICK=322378), getTickL (L по резервам бина), getSqrtPrice/getTickSqrtPriceAndL (цена по резервам и L — «дельты» бина). Цикла свопа по бинам в v2-common НЕТ (он в контракте пула v2-core) — отдельная задача | maverickprotocol/v2-common `TickMath.sol` (тест — TickMath.t.sol: testSqrtPrice 6 кейсов ≈1e-6, testGetTickLFixed) |
| `v2.rs` | getAmountOut / getAmountIn с комиссией из настройки | PancakeLibrary, UniswapV2Library (тест — v2-periphery) |
| `bin_price.rs` | Цена бина 128.128 | PriceHelper, Uint128x128Math.pow (тест — PriceHelper.t.sol) |
| `bin_swap.rs` | Обмен в пуле Bin по резервам бинов | BinPool.swap, BinHelper, FeeHelper |
