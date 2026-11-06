# Algebra Integral (THENA V3, DexSwap) — сырые ответы узла BSC, 06.11.2026

`probe.json` — записано серверным Claude (только чтение своего узла 127.0.0.1:8545) на ОДНОМ блоке 132036268:
- пулы thena-v3 `0xd1d3b61504ac6f3fe2aac12641cddaf1ec8bf91a` (BTCB/WBNB), `0x26399af833bca82c0c8c2ef4255b4939b26c39f5`
  (SINGULARRY/WBNB) и dexswap-bsc `0x68f820…` (CAT/WBNB, ликвидность 0);
- сырые eth_call: factory, token0/1, plugin, tickSpacing, liquidity, prevTickGlobal, nextTickGlobal, fee, globalState,
  decimals; обход ticks(t) по списку вверх/вниз; poolByPair фабрики;
- виртуальный квотер (contracts/VirtualQuoter.sol через state override, решение 186) на 1, 100, 10000 долларов в обе
  стороны — ответ (used, out);
- последние логи Swap и Fee (гистограмма topic0).

Факты из записи: Swap пула — 9 полей (topic0 0x121cb44e… = Swap(…, uint24 overrideFee, uint24 pluginFee)), Burn — с
pluginFee (0x932214d4…) — это integral-core 1.2.0/1.2.1 (теги v1.2-integral / v1.2.1-integral); poolByPair(token0,
token1) фабрики `0x30055f87…` возвращает ДРУГОЙ пул — эти пулы кастомные (customPoolByPair с деплоером).
Используется: tests/bsc_dex_algebra_integral_live.rs (решение 188).
