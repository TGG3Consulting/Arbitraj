# Curve StableSwap-NG (curve-bsc) — сырые ответы узла BSC, 14.11.2026

`probe.json` — записано Claude 14.11.2026 из своей вкладки браузера (eth_call / eth_getLogs к
https://bsc-rpc.publicnode.com, только чтение) на ОДНОМ блоке 133594967:
- пулы фабрики NG `0xd7E72f3615aa65b92A4DBdC211E296a35512988B`: rtUSQ/USDT `0xc2d55a4c…d563`, WBNB/USDT
  `0x47232815…04cb`, USDT/USDC/FDUSD `0xa5e0e464…8ec4` — N_COINS, A_precise, fee, offpeg_fee_multiplier,
  stored_rates, version, coins(k), balances(k), decimals() монет;
- реестр фабрики: get_coins(pool), is_meta(pool), get_pool_asset_types(pool);
- get_dy(i, j, dx) пула по всем парам индексов на 1, 100, 10000 единиц монеты входа (WBNB — /600);
- последние 6 логов пулов (TokenExchange, topic0 0x8b3e96f2…).
Используется: tests/bsc_dex_curve_ng_live.rs (решение 275).

`versions.json` — записано Claude 14.11.2026 из своей вкладки браузера (eth_call к https://bsc-rpc.publicnode.com,
только чтение) на ОДНОМ блоке 133600177 (0x7f693b1): все 21 пул curve-bsc из выдачи
https://api.geckoterminal.com/api/v2/networks/bsc/dexes/curve-bsc/pools (все страницы выдачи, ?page=1..3, — 21 пул) —
`version()` и `N_COINS()` пула, `is_meta(pool)` фабрики NG `0xd7E72f3615aa65b92A4DBdC211E296a35512988B`; для пулов не
версии v7.0.0 (и для WBNB/USDT для сравнения) — `get_coins(pool)` фабрики NG и `factory()` пула. Сырые ответы
JSON-RPC (result или error). Используется: tests/bsc_dex_curve_ng_live.rs (`versions_of_all_curve_bsc_pools`).

| Пул | Пара (GeckoTerminal) | version() | N_COINS | is_meta | factory() пула (если не фабрика NG) |
|---|---|---|---|---|---|
| `0x47232815af999bc552bc323e8030346c55fc04cb` | WBNB / USDT | v7.0.0 | 2 | false |  |
| `0xa5e0e46462970c9ee8c2ecadcde254c483748ec4` | FDUSD / USDT / USDC | v7.0.0 | 3 | false |  |
| `0xb84637ab9be835580821a67823f414ffd0bbf625` | bnbUSD / Sigma | v2.1.0 | откат | false | 0x98ee851a00abee0d95d08cf4ca2bdce32aeaaf7f |
| `0x8c6adffd84bb5c6a811fce6c9b40f9922a43af49` | SPCXB / SPCXon | v7.0.0 | 2 | false |  |
| `0xe6e2905f54baf7625f4943b74c50338362741cd4` | bnbUSD / USDT | v7.0.0 | 2 | false |  |
| `0xdbf96720976f8e54411e283e4cf1175dcf69ef43` | USDFI / USDC | v7.0.0 | 2 | false |  |
| `0x1c334db2dc8d7bbed62dced504fe1fe277d4264f` | frxUSD / USDu | v7.0.0 | 2 | false |  |
| `0xed55ff3fcb43249838a04a9707ff2c4b825507ff` | bnbUSD / U | v7.0.0 | 2 | false |  |
| `0xebafb74a98ba14430eedc716c0034123c3040ca4` | ezETH / ETH | v7.0.0 | 2 | false |  |
| `0x51d2d9f77661a63a4f07ff8bd7b1fc6cb227c5c4` | USDT / USDC | v7.0.0 | 2 | false |  |
| `0xc2d55a4c029525439d4220f5233ed949e501d563` | rtUSQ / USDT | v7.0.0 | 2 | false |  |
| `0xeab01b55c1fb144eb58869cd5c643820e7754c6b` | USDC / USDT | v7.0.0 | 2 | false |  |
| `0x93a3141f6a6d9239be5d5402bce1b3b2d1e83387` | rtUSQ / USDT | v7.0.0 | 2 | false |  |
| `0x5f0985a8aad85e82fd592a23cc0501e4345fb18c` | USDFI / ETH | v2.0.0 | откат | false | 0x98ee851a00abee0d95d08cf4ca2bdce32aeaaf7f |
| `0x95acf99a46e95cbfb1873344a56b5ee7d6881307` | USDFI / HAY | v7.0.0 | 2 | false |  |
| `0x478ef60aed6f4934712832cde7f1ff1c339e1299` | COOKIE / Cake / U,Z,E | v2.0.0 | откат | false | 0xc55837710bc500f1e3c7bb9dd1d51f7c5647e657 |
| `0x229d7255d6eaa27c73883c17ca3ff50a55186d81` | ZEST / USDT / EV | v2.0.0 | откат | false | 0xc55837710bc500f1e3c7bb9dd1d51f7c5647e657 |
| `0x17596fba3142eae0bf5a9d28211c9b94ad947c73` | frxUSD / USDT | v7.0.0 | 2 | false |  |
| `0xa1174d3af66ad2fd7c6d5c7b458b6da38988cd56` | HAY / USDT / USDC | v7.0.0 | 3 | false |  |
| `0x2b66f9470d35c985700724b4fc4fbe035c88c8f2` | USDFI / USDT | v7.0.0 | 2 | false |  |
| `0xc62e191402099bc364437a2a9f08bd7223046894` | PHPT / USDC | v2.1.0d | откат | false | 0x98ee851a00abee0d95d08cf4ca2bdce32aeaaf7f |

У пулов не v7.0.0 `get_coins` фабрики NG пуст (пул не из её реестра), `factory()` пула — другой контракт
(0x98ee851a…f7f или 0xc5583771…e657), `N_COINS()` откатывается: проверка вида `curve_stableswap_ng` их не берёт.
