# SquadSwap V3 — сырые ответы узла BSC, 15.11.2026 (UTC 14.11)

`probe.json` — записано Claude из своей вкладки браузера (eth_call / eth_getLogs к https://bsc-rpc.publicnode.com,
только чтение) на ОДНОМ блоке **133618816** (`block_hex`). Ответы узла — как есть (`0x…` — result; `{"error": …}` —
ответ с ошибкой). Решение 281. Используется: tests/bsc_dex_squadswap_v3_live.rs.

Адреса (официальная страница https://docs.squadswap.com/platform/developers/smart-contracts/bsc-contracts.md):
SquadV3Factory `0x10d8612D9D8269e322AB551C18a307cB4D6BC07B`, QuoterV2 `0xAF78c3C4100070337DBa8C79065339CfB77CFe7F`,
TickLens `0x7C9C9220B563A09167943fd327d50e858C0A6373`. Менеджер комиссии `0x391eaa90f931c6330132efe6c73ebdf77d782ef5`
— ответ `poolManager()` фабрики на этом блоке (`factory_calls.poolManager`). Пулы — из GeckoTerminal
(https://api.geckoterminal.com/api/v2/networks/bsc/dexes/squadswap-v3-bsc/pools): Cake/WBNB
`0x9fb9815ca1417b06466aaa86521a28ba83d0bd33`, LTC/WBNB `0xc144c2107f7882ed31bb419977435b00a8aa8c53`, USDT/WBNB
`0xa95d87b443e1929465931611b7951b92c1746dae`.

Что в записи:
- `factory_calls`: `poolManager()`; `swapFee(0x0, 0)` (селектор 0xd35fc6af, проба fee_probe.rs) — откат
  «execution reverted» (у SquadV3Factory функции swapFee нет).
- `pools[].calls`: fee() (200 / 1500 / 200), defaultFee() (100 / 500 / 100), slot0 (7 слов), liquidity, tickSpacing
  (1 / 10 / 1), factory(), token0/1, lmPool(); `getPool_fee` — getPool(token0, token1, fee()) фабрики = 0;
  `getPool_defaultFee` — getPool(token0, token1, defaultFee()) = сам пул; `manager_getFee` — getFee(pool, defaultFee)
  менеджера = fee().
- `pools[].words`: TickLens.getPopulatedTicksInWord по словам ±10 от слова текущего тика.
- `pools[].quoter`: на 1 / 100 / 10000 $ (сумма — по цене GeckoTerminal в момент записи, `price_usd` token0/token1) в
  обе стороны: `quoterV2_defaultFee` — QuoterV2.quoteExactInputSingle с fee = defaultFee (amountOut, sqrtPriceX96After,
  initializedTicksCrossed, gasEstimate); `virtual` — виртуальный квотер (contracts/out/VirtualQuoter.bin-runtime со
  squadV3SwapCallback, sha256 b6914455…19b7) через state override по адресу keccak256("arb.virtual_quoter")[12..] =
  `0xc7e4bba32c47c48b5cddf6c16cd234b3713f415a` — (used, out). Выходы совпадают во всех 18 случаях; USDT → WBNB на
  10000 $ — пул взял не весь вход (used < amountIn).
- `pools[].quoter_with_fee`: QuoterV2 с fee = fee() на 1 $ в обе стороны — откат (пул по этому ключу у фабрики не
  найден).
- `swap_logs`: последние 3 лога Swap (topic0 0x19b47279… — PancakeSwap V3, 9 полей) трёх пулов за блоки
  133614816–133618816 (все — LTC/WBNB).

Поиск событий менеджера (тот же день, eth_getLogs частями по 50000 блоков, адреса менеджера и фабрики, topic0
SetCustomFee 0xd444e1b1…, ToggleCustomFee 0xeebb2a3b…, UpdatePoolManager 0x14d37c29…): в блоках 129918817–133618816
событий НЕТ; раньше узел не отдаёт («Archive requests require a personal token»). Своя комиссия у записанных пулов
выставлена до блока 129918817 — живого лога SetCustomFee в доступной истории нет (факт).
