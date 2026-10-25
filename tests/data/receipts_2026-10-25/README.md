# tests/data/receipts_2026-10-25 — квитанции блоков BSC (решение 68)

`calls.json` — что записывать: наши пулы CAKE/ETH (V2/V3 — адреса, Infinity CL / Uniswap V4 — PoolId у менеджеров) из
tests/data/node_calls_3_2026-10-24.json, topic0 событий создания пула, размер скана и окна; рекордер —
`tests/recorders/record_receipts.js` (консоль браузера).

`responses.json` — живая запись публичного узла `https://bsc-dataseed.bnbchain.org` (техлид, 2026-10-25, файлы по коммиту
abcd3f1, sha256 сверены): latest 129702257, окно 129702236–129702239 (наших логов 2/4/0/1: V3 Swap ETH, V2 Sync/Swap ETH,
V3 CAKE), блок создания пула 129702218 (PairCreated фабрики PancakeSwap V2); заголовки eth_getBlockByNumber(n, false)
блока перед окном, блоков окна и блока создания; 11 результатов, ошибок 0.
