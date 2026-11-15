# Nomiswap Stable (nomiswap_stable) — сырые ответы узла BSC, 15.11.2026

Записано Claude 15.11.2026 из своей вкладки браузера (JSON-RPC к https://bsc-rpc.publicnode.com, только чтение) на
ОДНОМ блоке 133650877 (timestamp 1794707539).

`probe.json`:
- фабрика NomiswapStableFactory `0xc6b7ee49d386bae4fd501f2d2f8d18828f1f6285`: allPairsLength = 3, allPairs(0..2);
  getPair(token0, token1) каждой пары;
- пары (NomiswapStablePair): USDT/USDC `0x1e40…60f1`, USDT/BUSD `0x3a66…da87`, USDC/BUSD `0xca25…a7ec` — factory(),
  token0(), token1(), getReserves() (сырой ответ), swapFee(), getA(), devFee(), token0/1PrecisionMultiplier(),
  eth_getStorageAt(слот 7: initialA/futureA/initialATime/futureATime/swapFee — раскладка Sourcify storageLayout),
  decimals() монет;
- getAmountOut(address tokenIn, uint256 amountIn) (селектор 0xca706bcf) — по 4 суммы в каждую сторону (числа —
  десятичная запись слова ответа);
- два лога пары USDT/USDC (Sync 0x1c411e9a…, Swap 0xd78ad95f…; у Swap — данные в десятичной записи).

`exec_sim.json` — проба шага NOMI_STABLE (вид 12) ArbExecutor v2 на этом блоке через eth_call с подменой кода
(state override): исполнитель получил ровно getAmountOut пары; тот же маршрут шагом SOLIDLY (вид 3) — откат.

Исходники: Sourcify full match chain 56 — пары (все три — один и тот же NomiswapStablePair.sol, solc 0.8.15) и
фабрика NomiswapStableFactory. Используется: tests/bsc_dex_nomiswap_stable_live.rs (решение 284).
