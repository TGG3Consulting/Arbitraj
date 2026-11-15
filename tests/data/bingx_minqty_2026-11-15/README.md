# tests/data/bingx_minqty_2026-11-15 — пределы пар BingX и цены, 15.11.2026 (решение 293)

Публичные ответы BingX без ключа (только GET), получены 2026-11-15 около 11:40 UTC инструментом WebFetch (прямой
`curl` из среды разработки к `open-api.bingx.com` закрыт прокси — CONNECT 403). Зачем: показать фактом, у скольких пар
minQty × цена больше minNotional, и проверить код решения 293 на ответе по SHOPINX-USDT.

## Как записано (важно)

WebFetch отдаёт ответ через модель-пересказчик. Поэтому:
- **Дословно (JSON как есть)** пришли: `symbols_SHOPINX-USDT.json`, `symbols_BNB-USDT.json`, `symbols_MUBARAK-USDT.json`;
  у `book_ticker_SHOPINX-USDT.json`, `book_ticker_BTC-USDT.json`, `book_ticker_CAKE-USDT.json` — дословный массив `data`
  (обёртка `code` / `timestamp` — из списка «ключ = значение» того же ответа).
- **Остальные файлы — перенос полей ответа** «ключ = значение» (WebFetch перечислил все ключи с точными цифрами, полный
  JSON дословно не выдал): тот же набор и порядок ключей, что у дословного SHOPINX; у bookTicker цены и объёмы — строками,
  как в дословных ответах bookTicker.
- Кэш WebFetch: ответы ticker/24hr и bookTicker по BTC, DOGE, CAKE без параметра `timestamp` в начале запроса пришли
  СТАРЫМИ (поле `timestamp` ответа — сентябрь–октябрь 2026). Поэтому все записанные запросы — с официальным
  необязательным параметром `timestamp` первым (docs-v3 «Spot trading symbols», параметры: symbol, timestamp, recvWindow),
  и у каждого bookTicker поле `timestamp` ответа проверено: 1794742792120 … 1794742997401 (15.11.2026 11:39:52 …
  11:43:17 UTC). У ответа symbols поля времени нет.

## Файлы

| Файл | Запрос |
| --- | --- |
| `symbols_<ПАРА>.json` | `GET /openApi/spot/v1/common/symbols?timestamp=1794742830000&symbol=<ПАРА>` |
| `book_ticker_<ПАРА>.json` | `GET /openApi/spot/v1/ticker/bookTicker?timestamp=…&symbol=<ПАРА>` |

Пары: SHOPINX, CAKE, TWT, SFP, BTC, DOGE, BNB, MUBARAK, BANANAS31 (все к USDT, status 1). Запрошена ещё XVS-USDT:
symbols — status 0, bookTicker — код 100204 «symbol is not found.» (в выборку не вошла).

## Что показывают (minQty × bidPrice против minNotional)

| Пара | minQty | bid | minQty × bid, USDT | minNotional | Во сколько раз |
| --- | --- | --- | --- | --- | --- |
| SHOPINX | 57505 | 0.00870 | 500.29 | 1 | ×500 |
| BTC | 0.0001826 | 65514.01 | 11.96 | 0.5 | ×23.9 |
| BNB | 0.01597 | 598.52 | 9.56 | 1 | ×9.6 |
| CAKE | 2.672 | 1.627 | 4.35 | 1 | ×4.3 |
| DOGE | 67.4 | 0.05919 | 3.99 | 1 | ×4.0 |
| SFP | 12.01 | 0.1758 | 2.11 | 1 | ×2.1 |
| TWT | 4.52 | 0.3500 | 1.58 | 1 | ×1.6 |
| BANANAS31 | 168 | 0.0045600 | 0.77 | 1 | ×0.77 |
| MUBARAK | 9.84 | 0.02885 | 0.28 | 1 | ×0.28 |

Итог: у 7 из 9 пар minQty × цена больше minNotional (formula docs-v3 «minQty = minNotional / price» не выполняется ни у
одной). maxQty × цена против maxNotional: у CAKE 10689.471 × 1.627 ≈ 17 392 < 173 228 — maxQty строже maxNotional; у
SHOPINX maxNotional 1029 строже (maxQty × цена ≈ 5 млн).

Чужие продажи SHOPINX на BingX 13–15.11 по 8 362 … 11 756 монет (≈100 USDT, публичная лента исполнений) прошли при
minQty 57 505 — разбор в docs/DECISIONS.md, решение 293.

Кто читает: tests/bingx_minqty_293.rs.
