# Maverick V2 на BSC — источники математики обмена (решение 122 F, 03.11.2026)

Ветка `router/m2`. Итог: **свой расчёт — порт команды Maverick** (решение Тиграна 03.11: «Порт команды + сверка с
квотером»), а каждый обмен через пул Maverick в сделке сверяется с `MaverickV2Quoter.calculateSwap` на своём узле.

## Почему порт, а не официальный исходник

- Исходник пула (`MaverickV2Pool.sol`, `poollib/SwapMath.sol`, `Bin.sol`, `Delta.sol`, `Twa.sol`) — в приватном
  монорепо `maverickprotocol/maverick-v2` (папка `v2-amm/contracts`; ревизии 47287a62, 175f8c39, 23cf815e — по аудиту
  Omniscia). `v2-core`, `v2-supplemental`, `v2-router` — 404. Пул и фабрика не проверены в BscScan / Etherscan / zkSync.
- Открыто и официально: `maverickprotocol/v2-common` d91e8714 (Math.sol, TickMath.sol, PoolLib.sol, Constants.sol,
  интерфейсы), `v2-interfaces` f9f4f3eb, `maverick-v2-examples` 68e4d2fe; проверенные в эксплорере Arbitrum
  `MaverickV2Quoter` (0xb40AfdB8…, тот же адрес на BSC) и `MaverickV2PoolLens` (0x942646b0…, 0x56eFfDD5…).
  Цикла обмена в них нет: квотер считает через `try pool.swap(...)` + откат (MaverickV2Quoter.sol L2114-L2134).
- Whitepaper V2 (draft, docs.mav.xyz) описывает шаги обмена (с. 9-13), но без округлений и без формулы комиссии из
  параметра `fee` — по нему одному точного расчёта не написать.
- Порт: `VeloraDEX/paraswap-dex-lib` 4f74b83bb14d9b2c951a7cda673f0cca29dcad81, `src/dex/maverick-v2/maverick-math/` —
  написан командой Maverick (первый коммит a1f86b88 — Trung Dinh, правка 2108e064 — Bob Baxley / gte620v, Maverick).
  Сверка: `KyberNetwork/kyberswap-dex-lib` e76294c4 (Go, `pkg/liquidity-source/maverick/v2/pool_simulator.go`
  L244-L283 — тот же swapTick, ссылки на paraswap) и `@yldfi/maverick-v2-math` 63d95bc2 (заявляет ±1 wei к квотеру).

## Каждый элемент расчёта → источник

| Элемент | Наш файл | Источник |
|---|---|---|
| Фикс-точка D18, mulDivDown/Up одинарной ширины, divUp/Down, mulDown, invFloor/Ceil, clip, scale, ammScaleToTokenScale, tokenScaleToAmmScale | math/maverick_math.rs (был) | v2-common d91e871 contracts/libraries/Math.sol (официально); TS-порт basic-math.ts / maverick-math.ts совпадает |
| sqrt-цены краёв тика, L тика, sqrt-цена в тике | math/maverick_tick_math.rs (был) | v2-common TickMath.sol tickSqrtPrices / getTickL / getSqrtPrice / getTickSqrtPriceAndL (официально) |
| Доля протокола в комиссии | math/maverick_swap_math.rs `net_of_protocol_fee` | maverick-swap-math.ts L8-L19 |
| Вход тика на заданный выход | `input_for_output` | maverick-swap-math.ts L21-L42 |
| Обмен в тике, точный вход (комиссия с входа: `mulDown(in, 1e18−fee)`, feeBasis, остаток, выход по входу) | `exact_in` | maverick-swap-math.ts L44-L122 |
| Обмен в тике, точный выход | `exact_out` | maverick-swap-math.ts L124-L176 |
| Цикл: предел тика (Beyond Swap Limit), точный выход > резерва, масштаб входа, сумма шагов, округление итога | math/maverick_swap.rs `swap` | maverick-pool-math.ts estimateSwap L71-L137, combine L60-L69 |
| Шаг цикла: pastMaxTick, пропуск пустых тиков, L и цена тика, выработка тика → ±1 | `swap_tick` | maverick-pool-math.ts swapTick L633-L698, tickSqrtPriceAndLiquidity L611-L631; maverick-delta-math.ts L16-L26 |
| Изменение резервов тика после шага | `swap_tick` (allocate) | maverick-pool-math.ts allocateSwapValuesToTick L704-L727 |
| MVP-03M (Omniscia, acknowledged): несколько пустых тиков подряд и упор в предел — активный тик откатывается только на ±1 от прежнего | `swap_tick` L648-L651 → тест `mvp_03m_rollback_only_one_tick` | maverick-pool-math.ts L642-L651; Go-порт pool_simulator.go L262-L268 — так же |
| Перенос бинов (moveBins) и TWAP — ПОСЛЕ обмена, на суммы обмена не влияют | не считаются | maverick-pool-math.ts swap L201-L216, moveBins L229-L298; whitepaper §2.5 с. 12-13 |
| Направление тиков: вход tokenA — вверх, tokenB — вниз; без предела — int32 max/min | swap, quote/quoter_maverick.rs | maverick-v2-examples README L145-L148; IMaverickV2Pool.sol L102-L104 |
| Тики около активного одним вызовом, пустые не возвращаются | pools/load_maverick.rs | MaverickV2PoolLens (проверенный, Arbitrum 0x942646b0…) L3533-L3567 |

`fractionalPart` / `computeEndPrice` (swap-math L178-L198) нужны только TWAP — не портированы.

## Своё (не из порта)

- Тик вне прочитанного диапазона (активный ± `dex.tick_radius`) неизвестен: обмен останавливается, расчёт частичный
  (`complete = false`) — такой обмен не исполняется (exec/precheck.rs `quoter_now` — «не хватает ликвидности»).
- Переполнения, которые в Solidity 0.8 откатились бы (checked-арифметика), → `None` (TS bigint без предела).
- Глубина в полосе (решение 73): тот же цикл без комиссии до тика границы полосы (quote/tick_bin_spot.rs).
- Цена для размера пробы: sqrtPrice² активного тика (официальный TickMath).

## Предохранители (решение Тиграна)

1. Перед каждой сделкой через исполнитель (exec/executor/trade.rs `prepare`) и в сухой сделке — каждый хоп через пул
   Maverick сверяется с `MaverickV2Quoter.calculateSwap(pool, сумма хопа, tokenAIn, false, int32 max/min)` на своём узле
   (exec/precheck_quoter.rs → precheck::quoter_now). Расхождение больше `dex.quoter_max_diff_pct`, неполный расчёт или
   откат квотера — сделки нет. Цикл сверки (check/quoter_cycle.rs, решение 73) исключает такой пул из котировок.
2. Тест порта против записанных ответов квотера — `tests/bsc_dex_maverick_family.rs`
   `maverick_port_matches_live_quoter`, `#[ignore]` до записи (что записать — tests/README.md).

## Что не подтверждено (вопросы / проверить на узле)

- Байткод квотера на BSC = проверенному на Arbitrum (сверить `eth_getCode` 0xb40AfdB8… на BSC и Arbitrum).
- PoolLens на BSC (0x6A9EB38D…) — не тот адрес, что проверен на Arbitrum; поведение `getTicksAroundActive` взято с
  проверенного 0x942646b0… — сверить на записи (тики только непустые, диапазон активный ± радиус).
- `lookup(tokenA, tokenB, 0, N)` при N больше числа пулов пары — откат или обрезка? (пример maverick-v2-examples
  javascript/src/main.ts вызывает `lookup(0, 5000)` — другая перегрузка). Проверить на узле.
- Исполнитель вызывает пул без предела тика (int32 max/min, contracts/ArbExecutor.sol) — при нехватке ликвидности пул
  крутит цикл до нехватки газа (README L150); от этого защищает сверка порта/квотера и симуляция перед отправкой.
