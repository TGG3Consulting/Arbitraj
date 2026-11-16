# Крючки: карта тиков, половины protocolFee, пакет Multicall3 — сырые ответы узла BSC, 17.11.2026

Досъёмка серверного ИИ по промптам из docs/notes/t156-hook-pools-quoter-pause-2026-11-16.md (только чтение; свой узел
BSC geth 1.8.2, старый сервер arb-node; транзакций не отправлялось). Адреса — из настроек бота: CLPoolManager
0xa0FfB9c1CE1Fe56963B0321B32E7A0302114058b, CLQuoter 0xd0737C9762912dD34c3271197E362Aa736Df0926, Multicall3
0xcA11bde05977b3631167028862bE2a173976CA11, BinPoolManager 0xC697d2898e0D09264376196696c51D7aBbbAA4a9, StateView
0xd13dd3d6e93f276fafc9db9e6bb47c1180aee0c4. Настройки бота на момент снятия: dex.tick_words = 2,
dex.tick_read_band_pct = 70 (bsc_dex).

Здесь — только `raw_rpc.json` (как прислал серверный ИИ, без изменений; sha256
3b7e45b94f9528629284c51603b75871fec94eff2373f1332a91e63fa7069803): все eth_call п.1 и п.3 целиком
(`{t, method, params, result | error}`).

Не скопированы (тестам не нужны): `pools_ticks.json` (сводка п.1 из raw_rpc.json, sha256
8648b542eb49bc3d31f230623c29bf6c1211c9a889d290ed736be4a2567d28a1), `scan_part2_result.json` (итог п.2, sha256
36e6b657cf6802ef9270675512b729776a8fe84ec0be398840baaafc2f39ee5d), `raw_rpc_scan_part2.json` (3 825 255 байт, sha256
5f30b8bc1ca8f79538721280a001a4aee28a84ec0b4964d07a93a0830d0208f1 — 7939 getSlot0 на блоке 134000220: CLPoolManager
1603, StateView 6334, BinPoolManager 1; перепроверено скриптом: ни у одного ответа половины protocolFee не различаются),
`README.md` серверного ИИ (sha256 91d6d2aba0da2b9a6be5c1ba862d8f3b6a74638a16a37ff40ad0c1076b007ea5).

- **П.1 (блок 134000292):** пулы Infinity CL GAIB 0x5c1b4d20…, BTCMT 0x62166b8a…, CAKE 0xc528f2c0… — poolIdToPoolKey,
  getSlot0, getLiquidity, getPoolBitmapInfo по словам «слово тика ± 10», getPoolTickInfo всех установленных тиков,
  CLQuoter.quoteExactInputSingle (hookData 0x, без from) на 1, 20, 100 USDT в обе стороны.
- **П.2 (блок 134000220):** пулов с разными половинами protocolFee с USDT в паре нет (CL 1603, Bin 1, Uniswap V4 6334).
- **П.3 (блок 134000293):** RIVER, CAKE 0xc6104625…, BLESS — getSlot0, getLiquidity, квотер на 20 USDT в обе стороны
  одним Multicall3.aggregate3 (allowFailure = true) и теми же 12 вызовами по отдельности (записаны сразу после пакета).

Используется: tests/bsc_dex_hook_pools_ticks_1117.rs (п.1), tests/bsc_dex_hook_ladder_1117.rs (п.1, решение «335 (лесенка влияния)»), tests/bsc_dex_hook_pools_batch_1117.rs (п.3); чтение — tests/hook_ticks_common.
Решение «335 (досъёмка 17.11)», разбор — docs/notes/t156-hook-pools-quoter-pause-2026-11-16.md.
