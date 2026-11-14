# solidly_fee_2026-11-14 — запись узла BSC для решения 273

Снято 14.11.2026 из браузера владельца (fetch к https://bsc-rpc.publicnode.com, только чтение, JSON-RPC пачками).
Значения — разобранные слова ответов `eth_call` (uint256 → десятичная строка, address → 0x…, bool → true/false).

## state.json — один блок 133593628 (все eth_call с тегом этого блока)

По DEX: адрес фабрики, вызовы фабрики, пулы.

| DEX | фабрика | вызовы фабрики | пулы (stable) | комиссия пула |
|---|---|---|---|---|
| usdfi | 0xb3863573d9f25e6a84895d4685a408db7a488416 | isPaused() = 0, maxGasPrice() = 5000000000, getFee(bool) и getFee(address,bool) — откат | 0x012773…a384 (нет, fee 2500), 0x4c8887…40ea (нет, fee 333), 0xaa2955…6d (да, fee 10000) | fee() пула (ddca3f43) |
| veplus | 0x5bcd9ee6c31def33334b255ee7a767b6eedcba4b | getFee(bool), getFee(address,bool) — откат | 0x5a4ee6…d375 (нет, 5), 0xd815aa…ee23 (да, 1; decimals 18/6), 0x8c53f7…b2a3 (да, 1) | swapFee() пула (54cf2aeb) |
| magicfox | 0xceda3234d7d5b36114d886682a399c6d126a03e0 | stableFee() = 3, volatileFee() = 24, getFee(bool), getFee(address,bool) — откат | 0x1dde57…f385 (нет), 0xeebaef…19f6 (да), 0xa72d2a…f65a (да; decimals 18/6) | — (фабрика по типу) |

У каждого пула: `metadata()` (392f37e9: dec0, dec1, r0, r1, st, t0, t1), `getReserves()` (0902f1ac), комиссия, и
`getAmountOut(amountIn, tokenIn)` (f140a35a) в обе стороны на суммах reserveIn/1000 и reserveIn/30 —
`[вход, токен входа, выход]`.

Пулы найдены перебором `allPairs(i)` фабрик (usdfi 171, veplus 26, magicfox 36 пар) на том же узле.

## sync.json — последний Sync(uint256,uint256) каждой DEX

`eth_getLogs` по пулам записи и topic0 Sync (cf2aa508…848a) окнами по 5000 блоков назад от головы 133594091:
адрес, блок, logIndex, хеш транзакции, topics, data, число Sync этого пула в блоке, `getReserves()` пары на блоке лога
(у magicfox — null: узел не отдал состояние старого блока). Последний Sync каждой пары раньше блока state.json —
её резервы в state.json равны данным Sync.

Тест: tests/bsc_dex_solidly_fee_273.rs.
