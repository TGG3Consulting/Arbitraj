# Пулы PancakeSwap Infinity CL с крючком — сырые ответы узла BSC, 16.11.2026

`rpc.json.zst` — выборка из записи серверного ИИ (только чтение) со своего узла BSC (geth 1.8.2, старый сервер
arb-node), снятие 2026-11-16 20:39:27 UTC. Исходный файл записи `raw_rpc.json` (11 200 640 байт,
sha256 3366b4aa773283bc7fec8d918cb0b5ca356322a608bd230cb3b053b27d940b52) — все eth_call / eth_getLogs / eth_getCode
целиком; сюда взято только нужное тестам, JSON-массив тех же записей `{t, method, params, result | error}`, сжат zstd:

- все `eth_call` записи (1593) — как есть: на блоках H = 133993052, H+1, H+2 по каждому пулу CLPoolManager
  `poolIdToPoolKey` (0x0e2d484a), `getSlot0` (0xc815641c), `getLiquidity` (0xfa6793d5); CLQuoter
  `quoteExactInputSingle` (0x9938b8ed, hookData = 0x) — обе стороны (USDT → монета; монета → USDT на столько монеты,
  сколько стоят 20 / 1 USDT по цене slot0 того же блока), на 20 и 1 USDT, каждый вызов трижды: без from, from =
  Universal Router 0xd9C500DfF816a1Da21A48A732d3498Bf09dc9AEB, from = исполнитель (exec.executor_address);
- `eth_getLogs` CLPoolManager (topic1 = PoolId, блоки 133988053…133993052, события Swap и DynamicLPFeeUpdated) —
  в `result` оставлены последние 5 событий Swap пула и все прочие события; поле `cut` — сколько событий было
  (`swap_total`) и сколько оставлено;
- `eth_blockNumber`; `eth_getBlockByNumber` H…H+2 — в `result` только number, hash, timestamp (поле `cut`).

Файл `*.zst` попадает под .gitignore (сырые журналы) — добавлен в git принудительно (`git add -f`).

eth_getCode крючков и вызовы их view-функций в выборку не вошли (тестам не нужны).

Адреса — из настроек бота: CLPoolManager 0xa0FfB9c1CE1Fe56963B0321B32E7A0302114058b, CLQuoter
0xd0737C9762912dD34c3271197E362Aa736Df0926.

Пулы: 35 — с паузой сверки с квотером за 16.11 (журнал бота dex.quoter_pause, DEX pancakeswap-infinity-clmm), 2 — без
паузы, с крючком (CAKE 0xc6104625…, BLESS 0x6e398c95…). OPG 0x0636066c… и CYS 0x9cfea9ef… — без USDT в паре,
квотер по ним не спрашивался.

| PoolId (начало) | монета | fee ключа | protocolFee / lpFee slot0 | fee последних Swap |
|---|---|---|---|---|
| 0x62166b8a | BTCMT | 0x800000 (динамическая) | 1229100 / 500 | 838 |
| 0x5c1b4d20 | GAIB | 0x800000 | 1229100 / 500 | — |
| 0xaf120885 | CAKE | 0x800000 | 1229100 / 500 | 800 |
| 0xc6104625 | CAKE (без паузы) | 0x800000 | 1229100 / 0 | 3300 |
| 0x6e398c95 | BLESS (без паузы) | 0x800000 | 1229100 / 500, ликвидность 0 | — |
| 0xc528f2c0 | CAKE | 100 | 200753 / 100 | — |
| 0xe3a43b0d | BTC | 100 | 200753 / 100 | 149 |
| остальные 30 | BTW, PIEVERSE, ACU, RAVE, CLO, RIVER, DNA, IRYS, ZEST, BILL, CNPY, OPG, BSB, KITE, GWEI, GRVT, CYS, BASED, LAB, ZAMA, AEON, FIGHT, BTG, KGEN, ARX, DEBIT, BEAT, QAIT, SHARE, GUA | 67 | 131104 / 67 | 99 |

protocolFee — две половины по 12 бит (infinity-core ProtocolFeeLibrary.sol): 131104 = 0x020020 → 32 / 32,
200753 = 0x031031 → 49 / 49, 1229100 = 0x12C12C → 300 / 300.

Используется: tests/bsc_dex_hook_pools_1116.rs, tests/bsc_dex_hook_pools_1116_rules.rs (общее чтение —
tests/hook_pools_common). Решение «335 (записи узла)», разбор — docs/notes/t156-hook-pools-quoter-pause-2026-11-16.md.
