# tests/data/service_tax_2026-10-25 — проба переводов USDT (решение 67)

`calls.json` — точный запрос eth_simulateV1, собранный кодом бота (check/service_tax.rs `calls`; тест
`tests/bsc_dex_service_tax.rs` сверяет файл с кодом). Источник USDT — пара PancakeSwap V2 WBNB/USDT
0x16b9…0daE (getPair в tests/data/native_2026-10-25), кошелёк — 0x8894…D4E3 той же записи, сумма — 5·10^18 единиц.

`responses.json` — ещё не записан: ответ узла на `calls.json` и decimals() USDT, рекордер
`tests/recorders/record_service_tax.js` (консоль браузера; из песочницы узел BSC недоступен).
