# SheepDex — сырые ответы узла BSC, 14.11.2026

`probe.json` — записано Claude 14.11.2026 из своей вкладки браузера (eth_call / eth_getLogs к
https://bsc-rpc.publicnode.com, только чтение) на ОДНОМ блоке 133593936:
- пулы SpePool `0x197880438436c426392570cb4782432ec9135f62` (BTCB/BUSD, 0,2 %) и
  `0x187f9b3aa52f3ce82334bea80ffe769e1c134065` (USDT/WBNB, 0,2 %): slot0, liquidity, fee, tickSpacing, factory,
  token0/1; getPool(token0, token1, fee) фабрики SpeFactory `0x571521f8c16f3c4ed5f2490f19187ba7a5a3cbdf`;
- `swapFee(адрес, 0)` фабрики для нулевого адреса и адреса виртуального квотера (оба 0 — своей комиссии нет);
- тики: Uniswap TickLens `0xD9270014D396281579760619CCf4c3af0501A47C` getPopulatedTicksInWord по словам ±6 от
  текущего;
- виртуальный квотер (contracts/out/VirtualQuoter.bin-runtime со swapCallback, решение 274) через state override по
  адресу keccak256("arb.virtual_quoter")[12..] — ответ (used, out) на 1, 100, 10000 долларов в обе стороны;
- последние 6 логов пулов (Swap Uniswap V3, topic0 0xc42079f9…).
Используется: tests/bsc_dex_sheepdex_live.rs (решение 274).

`swap_fee_probe.json` — записано Claude 14.11.2026 из своей вкладки браузера (eth_call к
https://bsc-rpc.publicnode.com, только чтение) на блоке 133600177 (0x7f693b1): `swapFee(0x0, 0)` и `swapFee(0x0, 500)`
(селектор 0xd35fc6af) к фабрикам Uniswap V3 BSC `0xdB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7`, PancakeSwap V3 BSC
`0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865` (адреса — из настроек бота dex.factory, id uniswap-bsc и
pancakeswap-v3-bsc) и SheepDex `0x571521f8c16f3c4ed5f2490f19187ba7a5a3cbdf`. Uniswap и PancakeSwap — откат
(`execution reverted`), SheepDex — ответ uint24 (0 и 500 = originalFee). Сырые ответы JSON-RPC. Используется:
tests/bsc_dex_sheepdex_live.rs (`swap_fee_probe_tells_sheepdex_from_uniswap_and_pancake`, защита fee_probe.rs).
