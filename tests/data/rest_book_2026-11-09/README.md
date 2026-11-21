# Примеры ответов стакана REST из официальной документации (решение 238, 09.11.2026)

Живых ответов этих бирж из среды разработки нет (прокси закрывает адреса бирж); файлы — примеры ответов из официальной
документации, без изменений чисел (комментарии примеров убраны):

- `xt_depth_doc.json` — XT, GET /v4/public/depth: github.com/xt-com/xt4-api, ветка main, `_market/3depth.md`
  (Response; обёртка {rc, mc, ma, result} — как в примере).
- `phemex_orderbook_doc.json` — Phemex, GET /md/orderbook?symbol=sBTCUSDT: github.com/phemex/phemex-api-docs,
  `Public-Spot-API-en.md`, «Query Order Book» (Sample).
- `poloniex_orderbook_doc.json` — Poloniex, GET /markets/{symbol}/orderBook:
  https://api-docs.poloniex.com/spot/api/public/market-data, «Order Book» (Example response).

Используются: tests/core_exec_book_238.rs (XT), tests/rest_book_238.rs (все три). Записать живые ответы на сервере
бота — тем же запросом с символом торгуемой пары (вопрос Poloniex о виде списка asks/bids — docs/TASKS.md).
