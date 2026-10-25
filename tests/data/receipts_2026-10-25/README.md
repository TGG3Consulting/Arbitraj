# tests/data/receipts_2026-10-25 — квитанции блоков BSC (решение 68)

`calls.json` — что записывать: наши пулы CAKE/ETH (V2/V3 — адреса, Infinity CL / Uniswap V4 — PoolId у менеджеров) из
tests/data/node_calls_3_2026-10-24.json, topic0 событий создания пула, размер скана и окна; рекордер —
`tests/recorders/record_receipts.js` (консоль браузера).

`responses.json` — живая запись публичного узла `https://bsc-dataseed.bnbchain.org` (техлид, 2026-10-25, файлы по коммиту
abcd3f1, sha256 сверены): latest 129702257, окно 129702236–129702239 (наших логов 2/4/0/1: V3 Swap ETH, V2 Sync/Swap ETH,
V3 CAKE), блок создания пула 129702218 (PairCreated фабрики PancakeSwap V2); заголовки eth_getBlockByNumber(n, false)
блока перед окном, блоков окна и блока создания; 11 результатов, ошибок 0.

`calls_2.json` / `record_receipts_2.js` — окно по нашим 13 PoolId Infinity CL и Uniswap V4: в 120 блоках 0 логов, запись не сохранена.

`calls_3.json` / `tests/recorders/record_receipts_3.js` → `responses_3.json` — живая запись того же узла (техлид, коммит 6e57d19,
sha256 сверены): окно 129703969–129703972 по логам CLPoolManager и PoolManager V4 с любым PoolId (17/18/10/13, наших V2/V3
0/2/2/0), поле `seen` — 21 лог менеджеров окна (менеджер, PoolId, topic0); блок создания пула 129703964; ошибок 0.
