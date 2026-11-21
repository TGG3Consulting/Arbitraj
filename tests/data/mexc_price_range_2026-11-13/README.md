# MEXC 30087 — живой ответ на лимитный ордер продажи (13.11.2026)

`order_30087.json` — сырое тело ответа MEXC на POST /api/v3/order из журнала бота (`core.op_error`, поле `error.raw`):
13.11.2026 21:13 UTC, три сделки FAR bsc_dex → mexc — лимитный ордер продажи (IMMEDIATE_OR_CANCEL) с худшей ценой
0.000460 при котировке продажи «средняя 0.000489, исполнимо всё». Кода 30087 в официальной таблице Error Code MEXC нет —
основание разбора (решение 268, код шлюза PriceOutOfRange) — этот живой ответ; правило коридора — фильтр
PERCENT_PRICE_BY_SIDE в exchangeInfo (https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/exchange-information).
Используется в tests/price_range_268.rs (`mexc_recorded_30087_is_price_out_of_range`).
