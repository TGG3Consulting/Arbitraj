# iZiSwap / Biswap V3 (iziswap-bsc, biswap-v3, biswap-v3-1) — сырые ответы узла BSC, 15.11.2026

Записано Claude 15.11.2026 из своей вкладки браузера (JSON-RPC к https://bsc-rpc.publicnode.com, только чтение и
eth_call с подменой кода — в сеть ничего не уходило; GeckoTerminal API v2 — список пулов DEX).

`probe.json` (блок 133668333, кроме отмеченного):
- фабрики трёх DEX: Sourcify (iZiSwapFactory — exact_match, BiswapFactoryV3 0x7c3d… — match, 0x4d17… — нет), реестр
  пулов: у iZiSwap `pool(address,address,uint24)` 0xbecbcc6a, у обеих фабрик Biswap — `pool(address,address,uint16)`
  0x0668c8a2 (чужой селектор — откат);
- пулы: tokenX, tokenY, fee, factory, leftMostPt, rightMostPt, pointDelta и ответ реестра фабрики (в обоих порядках
  токенов) — у всех четырёх пулов реестр = сам пул; сравнение кода пулов (biswap-v3 и biswap-v3-1 — один код, кроме
  32 байт метаданных; iZiSwap — другой код с теми же селекторами обмена и геттеров);
- `callback_probe` — какой обратный вызов зовёт пул (пробный контракт): swapX2YCallback 0x18780684 (платить x),
  swapY2XCallback 0xd3e1c284 (платить y) у всех трёх; вызывающий — любой контракт (белого списка нет);
- `event_topics` — topic0 записанных логов и PUSH32 в коде пулов;
- `quoteIzi_raw` — сырые ответы VirtualQuoter.quoteIzi (код решения 286) и выход пробного контракта на тех же суммах:
  out квотера = выход пробы во всех 12 ответах; used = amountIn, кроме края пула (used < amountIn); откат «dc» —
  ошибка узла.

`exec_sim.json` — шаг IZI (вид 14) ArbExecutor v2 через eth_call с подменой кода: получено ровно quoteIzi_out во всех
8 случаях (3 DEX, 4 пула, обе стороны); тот же маршрут видом V3 — откат.

Исходники: iZiSwap — github.com/izumiFinance/iZiSwap-core (коммит 3fe21eba, contracts/iZiSwapPool.sol, swapX2Y.sol,
swapY2X.sol, iZiSwapFactory.sol, interfaces/IiZiSwapCallback.sol); Biswap V3 — Sourcify chain 56: BiswapPoolV3
0x76008e4007ab26142dbeafee24d6113de47eb76b (exact_match), BiswapFactoryV3 0x7c3d53606f9c03e7f54abddffc3868e1c5466863
(match). Используется: tests/bsc_dex_izi_live.rs (решение 286).
