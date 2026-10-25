# tests/data/service_tax_2026-10-25 — проба переводов USDT (решение 67)

`calls.json` — точный запрос eth_simulateV1, собранный кодом бота (check/service_tax.rs `calls`; тест
`tests/bsc_dex_service_tax.rs` сверяет файл с кодом). Источник USDT — пара PancakeSwap V2 WBNB/USDT
0x16b9…0daE (getPair в tests/data/native_2026-10-25), кошелёк — 0x8894…D4E3 той же записи, сумма — 5·10^18 единиц.

`responses.json` — живой ответ узла `https://bsc-dataseed.bnbchain.org` на `calls.json` (коммит f16aa9e, sha256 сверен),
блок `0x7bb0bc5`, и decimals() USDT (18); записал техлид рекордером `tests/recorders/record_service_tax.js`
`{about, block, request, response, decimals}`. Все 9 вызовов — status 0x1; газ 23963/34503/23963/23963/34503/23963/
23963/51603/23963. Налог USDT: из пула, в пул, на адрес — 0.
