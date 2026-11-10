# binance_docs_2026-11-10 — примеры официальной документации Binance (решение 253)

Живых записей приватных ответов Binance в tests/data нет (из среды разработки api.binance.com закрыт прокси) — как
принято в проекте, тесты идут на примерах официальной документации (значения полей сверены 10.11.2026):

| Файл | Источник |
| --- | --- |
| `outbound_account_position.json` | https://developers.binance.com/legacy-docs/binance-spot-api-docs/user-data-stream — «Account Update», пример outboundAccountPosition в обёртке WebSocket API (`subscriptionId`, `event`); комментарии примера убраны |
| `withdraw_history.json` | https://developers.binance.com/legacy-docs/wallet/capital/withdraw-history — «Withdraw History (supporting network)», первая запись примера ответа (`transactionFee` — «transaction fee») |

Коды отказа вывода −4026 CAPITAL_WITHDRAW_USER_ASSET_NOT_ENOUGH («You have insufficient balance») и −4024
CAPITAL_WITHDRAW_USER_NO_ASSET («You don't have this asset») — https://developers.binance.com/en/docs/products/wallet/error-code.
Тексты отказов с номерами [031033] / [031031] — из хронологии сделок бота 10.11 (CYBER, WLFI).

Записать на сервере бота (venue.balance_events = да, сырой журнал data/raw/binance, соединение «account») кадры потока
аккаунта вокруг покупки и вывода — сверить, когда приходит outboundAccountPosition купленной монеты относительно ответа
вывода −4026.
