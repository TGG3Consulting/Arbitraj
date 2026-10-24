# tests/data/indexers_2026-10-24 — живые ответы индексаторов (машина Тиграна, 2026-10-24 13:20–13:22 UTC)

`arb_indexers_2026-10-24.json` — объект `{имя: {url, status, headers, at, body}}`, `body` — тело ответа строкой, как пришло:

| Имя | Запрос | Что внутри |
| --- | --- | --- |
| `gt_dexes_bsc_p1`, `gt_dexes_bsc_p2` | GeckoTerminal `GET /api/v2/networks/bsc/dexes?page=1/2` | 139 DEX сети BSC (100 + 39), `links.next` — `null` на второй странице |
| `gt_dexes_bsc_p3` | то же, `page=3` | HTTP 400 `{"errors":[{"status":"400","title":"expected :page in 1..2; got 3"}]}` — страница за пределами |
| `gt_pools_{CAKE,ETH,XRP,TWT,DOGE}_p1` | GeckoTerminal `GET /api/v2/networks/bsc/tokens/{адрес}/pools?page=1&include=base_token,quote_token,dex` | по 20 пулов; у пулов V4/Infinity `address` — PoolId (0x + 64 знака), у V2/V3 — адрес контракта; у ответа нет `links` |
| `ds_pairs_*` | DexScreener `GET /token-pairs/v1/bsc/{адрес}` | по 30 пар, одной страницей; второй источник — позже (решение 58) |

Адреса монет — как у BingX в сети BSC: CAKE `0x0E09…cE82`, ETH `0x2170…33F8`, XRP `0x1D2F…0dBE`, TWT `0x4B0F…8003`,
DOGE `0xbA2a…4C43`. Тесты — tests/bsc_dex_directory.rs.

`arb_node_responses_2026-10-24.json` — ответы публичного узла BSC `https://bsc-dataseed.bnbchain.org` (Chrome Тиграна,
2026-10-24 15:57 UTC, записал техлид) на вызовы `tests/data/node_calls_2026-10-24.json`, все на блоке `0x7b8a659`:
`{about, block, results: [{name, request: {method, params}, response}]}`, 225 записей в порядке списка; 49 — откаты
(`response.error` как вернул узел); последняя — Multicall3.aggregate3 всех 223 вызовов (вход совпал с нашим байт в байт).
Внутри: поля пулов CAKE и ETH (factory, token0, token1, fee, tickSpacing, lmPool), getPair PancakeSwap V2,
poolIdToPoolKey CLPoolManager, poolKeys и poolManager() PositionManager Uniswap V4. Тесты — tests/bsc_dex_node_verify.rs.
Второй проход (getPool фабрик V3 на том же блоке, eth_getLogs событий обмена) — `tests/data/node_calls_2_2026-10-24.json`,
ждёт записи.
