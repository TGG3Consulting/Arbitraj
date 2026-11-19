# Poloniex: быстрый стакан book_lv2 со страховкой (решение 424, 20.11.2026)

Источник: https://api-docs.poloniex.com/spot/websocket/market-data (Book, Book Level 2), прочитано 19–20.11.2026.

## Что сказано в офдоке

| Канал | Что даёт | Частота | Номера |
| --- | --- | --- | --- |
| book | снимок 5 / 10 / 20 уровней (`depth`) | «Order book is updated every 100ms» | `id` — «Record ID (SeqId)» |
| book_lv2 | «Receive a snapshot of the full 20 level order book», затем изменения (`action` snapshot / update) | не указана | `lastId` — id предыдущего сообщения; при несовпадении — «must re-subscribe to the channel» |

«How to maintain a local order book»: снимок — сбросить стакан; изменение `[price, quantity]` — количество > 0
заменить, 0 — удалить цену; новый снимок — сбросить снова. REST-снимок не нужен. Примеры офдоки: snapshot
lastId 10409 / id 10410; update lastId 10410 / id 10421 (id растёт не на 1, цепочку задаёт lastId).

## Как сделано

- Пара подписана на оба канала (две подписки, по одному символу в сообщении, очередь venue.ws_subs_per_sec).
- book_lv2 → `local_book.rs` (BTreeMap по сторонам, срез глубже 20 уровней, проверка пересечения).
- `fast_book.rs` выбирает стакан ядру: быстрый, пока синхронизирован; разрыв → последний снимок book (не старше
  venue.book_stale_ms и не старше уже отданного по createTime) или Invalidated; после снимка book_lv2 — снова быстрый.
- Переподписка book_lv2 пары после разрыва — `ws_book_sync.rs`, не чаще venue.ws_reconnect_pause_ms на пару.
- Сводка `venue.book_fast` — не чаще log.pool_events_lost_interval_s.

## Чего нет в документации (проверить на живой записи)

1. Частота book_lv2 (быстрее ли book на практике).
2. Общая ли последовательность id у book и book_lv2 — поэтому сверка снимка с быстрым стаканом только по содержимому.
3. Шлёт ли сервер изменение для уровня, вошедшего в первые 20 после удаления верхнего.
4. Ответ на подписку при уже действующей подписке — поэтому переподписка = отписка + подписка.
5. Сколько подписок выдерживает одно соединение (venue.ws_symbols_per_conn).
