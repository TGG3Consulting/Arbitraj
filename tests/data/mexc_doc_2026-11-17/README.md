# MEXC — пример ответа из официальной документации, 17.11.2026

`generate_deposit_address_example.json` — пример ответа «Generate deposit address (supporting network)»
POST /api/v3/capital/deposit/address со страницы
https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/generate-deposit-address-supporting-network (прочитана
17.11.2026, задача t165): массив из двух записей (USDT TRC20 с `"tag": null`, EOS с memo), поля и значения — как на
странице (тот же пример — в прежней документации mexcdevelop.github.io). Это пример документации, не живой ответ: живой
ответ бот пишет в сырой журнал сервера при первом создании адреса (`<data-dir>/raw/mexc/*.jsonl.zst`) — записать сюда же
(docs/TASKS.md, t165). Нужен тесту tests/mexc_deposit_addr_t165.rs (решение 379).
