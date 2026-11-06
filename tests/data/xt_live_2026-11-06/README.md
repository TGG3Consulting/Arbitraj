# XT.com — живой ответ GET https://sapi.xt.com/v4/public/symbol, 06.11.2026

`symbol_pepe.json` — result ответа с двумя парами без изменений: pepe_usdt (фильтр QUANTITY max = "1000000000000000000000000000000",
10^30 — больше предела Decimal ~7.9·10^28; из-за этого 06.11.2026 падал разбор всего справочника XT — подписка XT не
поднималась, «поле max не число») и btc_usdt (обычная пара). Поля baseCurrencyLogo убраны (ссылки на картинки).
Используется: tests/xt_symbols_live.rs (решение 190).
