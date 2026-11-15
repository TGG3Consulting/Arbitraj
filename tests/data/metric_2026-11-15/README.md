# Metric OMM (metric-bsc) — сырые ответы узла BSC, 15.11.2026

Записано Claude 15.11.2026 из своей вкладки браузера (JSON-RPC к https://bsc-rpc.publicnode.com, только чтение и
eth_call с подменой кода — в сеть ничего не уходило).

`probe.json` (блок 133656733):
- фабрика `0x2a53833cc95548cf52c7b159110e22d3a9018f32`: nextPoolIdx = 386 (пулы idxToPool(1..385)); isPool и poolToIdx
  четырёх пулов;
- getImmutables() пулов WBNB/USDT `0xf3eb…afd6`, NVDAB/USDT `0x7201…d117`, USDC/USDT `0xb3ad…0677`, QQQB/USDT
  `0xff28…676d` (24 слова — как кортеж SDK v1: factory, token0, token1, … , extension1..7, порядки хуков);
- getSellAndBuyPrices() (X64) — у QQQB откат PriceProviderFailed (0x46a14930);
- swaps_usdt_in — пробный контракт MetricProbe (eth_call с подменой кода; swap + оплата положительной дельты в
  metricOmmSwapCallback): выход на 10 / 1000 / 100000 USDT;
- logs_topic0 — topic0 всех 592 логов двух пулов за блоки 133653733…133656733 (имена событий не установлены);
- code_selectors — PUSH4 / PUSH32 селекторов в коде пула и фабрики; survey — обзор всех 385 пулов (getImmutables,
  расширения), проба обмена у 6 крупнейших пулов с USDT (блок latest ≈133650877…133656733).

`exec_sim.json` (блок 133656733): ответы VirtualQuoter.quoteMetric (код решения 285) = ответам пробного контракта;
шаг METRIC (13) ArbExecutor v2 решения 285 по NVDAB/USDT и WBNB/USDT получил ровно то же; неверный флаг zeroForOne —
откат IncorrectDelta (0xbe795921).

Исходники: исходник пулов и фабрики BSC НЕ опубликован (Sourcify — нет; репозиторий контрактов не найден; страница
документации docs.metric.xyz из этой среды не открылась). ABI — официальный SDK npm @nradko/metric-omm-sdk-v1@0.4.5
(репозиторий github.com/Metric-OMM/metric-sdk). Используется: tests/bsc_dex_metric_live.rs (решение 285).
