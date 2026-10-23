# venues/bsc_dex/exec — исполнение обмена

| Файл | Что делает |
| --- | --- |
| `calls.rs` | Данные вызова прямого обмена V2/V3 через роутер своей DEX (V2 — обычная или FOT-функция; PancakeSwap V3 — SwapRouter; Uniswap V3 — SwapRouter02.multicall с deadline) |

Отправка, V4 (Universal Router), проверка реально полученного баланса, трекер — шаг 1d.
