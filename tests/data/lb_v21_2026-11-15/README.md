# Liquidity Book V2.1 / LFJ (traderjoe-v2-1-bsc) — сырые ответы узла BSC, 15.11.2026

Записано Claude 15.11.2026 из своей вкладки браузера (JSON-RPC к https://bsc-rpc.publicnode.com, только чтение и
eth_call, в том числе с подменой кода и балансов — в сеть ничего не уходило; GeckoTerminal API v2 — список пулов DEX;
Sourcify v2 — исходники и сведения о развёртывании; GitHub lfj-gg/joe-v2@v2.1.0 — официальный исходник). Ответы
переписаны из страницы вкладки; совпадение переписанного с ответом вкладки проверено SHA-256 строки JSON (probe —
721cd057…bfa3fa, exec_state — 4bcec46e…aed3ab).

`probe.json` (блок 133716961, время блока 1794737287):
- фабрика LBFactory 0x8e42f2f4101563bf679975178e880fd87d3efd4e — Sourcify exact_match (src/LBFactory.sol, solc 0.8.10,
  runs 800, блок 27099340); getNumberOfLBPairs = 147, из них с ненулевыми резервами (getReserves) — 61;
- реализации пула: 0xee5a9009…86df (exact_match, блок 27099340) и 0x3e30fdae…2d8f (exact_match, блок 28439323); тексты =
  lfj-gg/joe-v2 тег v2.1.0 (у второй — pragma ^0.8.10 и исправление TreeMath.remove, LBPair.sol тот же; source_excerpt.txt);
- GeckoTerminal: id DEX `traderjoe-v2-1-bsc`, 3 пула — USDC/USDT 0xaec9…060c (58,36 USD), BTC.b/WBNB 0x717d…073d
  (428,57 USD), BTCBR/WBNB 0x3208…2f67 (62,31 USD): сумма 549,24 USD;
- пять пулов (три из GeckoTerminal и 0x722e…912f, 0x023f…3fd5 — с резервами обеих сторон): код клона (97 байт, адрес
  реализации), getFactory, getTokenX / getTokenY (у USDC/USDT и BUSD/USDT X — больший адрес), getBinStep, getActiveId,
  getStaticFeeParameters, getVariableFeeParameters, getReserves, getProtocolFees, decimals, getLBPairInformation фабрики
  (пул тот же), ВСЕ непустые бины (обход getNextNonEmptyBin до края в обе стороны, getBin); `quotes` — getSwapOut на 5 суммах
  в обе стороны (последняя — больше ликвидности: остаток входа > 0);
- `foreign` — настоящий пул PancakeSwap V2 USDT/WBNB 0x16b9…0dae: getFactory / getTokenX / getBinStep откатываются.

`exec_state.json` (тот же блок 133716961) — шаг LB (вид 18) ArbExecutor v2 решения 290 через eth_call с подменой кода
(contracts/out/ArbExecutor.bin-runtime, 16 212 байт, SHA-256 байтов be076146…54cbbc — проверено в вкладке): calldata
execute (как есть), received; getSwapOut тех же пулов на том же блоке (у маршрута — цепочкой). 10 вызовов прошли (9
одиночных шагов обеих сторон пяти пулов и маршрут BUSD → USDT → USDC), received = getSwapOut; 2 вызова со входом больше
ликвидности откатились LBPair__OutOfLiquidity (0xd36bfd88); шаг LB в пару PancakeSwap V2 — откат. Состояние пулов на этом
блоке — probe.json.

`source_excerpt.txt` — дословные выдержки v2.1.0 с номерами строк: LBPair (геттеры, getSwapIn, getSwapOut, swap, аргументы
клона, _getNextNonEmptyBin), PairParameterHelper, FeeHelper, Constants, BinHelper (getAmounts, received), PriceHelper,
Uint128x128Math.pow, Uint256x256Math (mulShift / shiftDiv), TreeMath (findFirstRight / findFirstLeft), LBFactory
(getLBPairInformation), ILBFactory, ILBPair (события).

Используется: tests/bsc_dex_lb_v21_live.rs (решение 290).
