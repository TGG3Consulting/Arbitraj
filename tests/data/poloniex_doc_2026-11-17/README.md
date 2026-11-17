# Poloniex — пример ответа из официальной документации, 17.11.2026

`new_currency_address_example.json` — пример ответа «New Currency Address» POST /wallets/address со страницы
https://api-docs.poloniex.com/spot/api/private/wallet (прочитана 17.11.2026, задача t165): поле `address` («the newly
created address») со значением, как на странице. Это пример документации, не живой ответ и не адрес владельца: живой
ответ бот пишет в сырой журнал сервера при первом создании адреса (`<data-dir>/raw/poloniex/*.jsonl.zst`, соединение
«rest /wallets/address») — записать сюда же (docs/TASKS.md, t165). Нужен тесту tests/poloniex_deposit_addr_t165.rs
(решение 379).
