# OKX: страховка стакана books5 (решение 429, задача t213, 20.11.2026)

Общая постановка Тиграна (20.11.2026): стакан для торговли — из самого быстрого потока; при пропусках торговлю не
останавливать — параллельно слушать поток снимков и сразу считать по нему.

## Что было

- Канал стакана — по `venue.depth_levels`: 5 → books5 (снимки), 400 → books (снимок 400 уровней + изменения раз в 100 мс,
  prevSeqId/seqId; `local_book.rs`).
- Разрыв prevSeqId, отказ подписки, неразборчивый кадр — пара недостоверна (Invalidated) и пересподписка; обрыв
  соединения — недостоверны все пары соединения до переподключения и новых снимков.

## Документация (https://www.okx.com/docs-v5/en/, прочитано 20.11.2026 пересказчиком, www.okx.com закрыт прокси)

- «Order book channel»: books — «400 depth levels will be pushed in the initial full snapshot. Incremental data will be
  pushed every 100 ms for the changes in the order book during that period of time»; books5 — «5 depth levels snapshot
  will be pushed in the initial push. Snapshot data will be pushed every 100 ms when there are changes in the 5 depth
  levels snapshot»; ts — «Order book generation time, Unix timestamp format in milliseconds»; prevSeqId — «Only
  applicable to books, books-rpi, books-l2-tbt, books50-l2-tbt»; у books5 в примере есть seqId, нет prevSeqId;
  checksum «Deprecated», 0.
- «Users can not simultaneously subscribe to books-l2-tbt and books50-l2-tbt/books channels for the same trading symbol»
  — про books + books5 запрета нет.
- books-l2-tbt / books50-l2-tbt (10 мс): «Only API users who are VIP4 and above in trading fee tier are allowed to
  subscribe» (иначе 64003); «Identity verification refers to Login», URL Path /ws/v5/public — канал не назван; раздел
  Login — без адреса; «Public channels -- No authentication is required». Вход на public-соединении не описан → режим не
  сделан (docs/TASKS.md).
- «Overview → WebSocket»: «Connection limit: 3 requests per second (based on IP)»; «The total number of
  'subscribe'/'unsubscribe'/'login' requests per connection is limited to 480 times per hour»; «it is recommended to
  subscribe through multiple websocket connections, with each of less than 30 channels».

## Как сделано

- Страховка — books5 на ОТДЕЛЬНЫХ соединениях `backup-N` с теми же группами пар (обрыв основного `market-N` её не
  задевает; у каждого соединения свой предел 480 запросов в час). Подключения обоих видов — одной очередью
  `venue.ws_connects_per_sec`. Только при канале books (при books5 быстрого потока нет — страховать нечем).
- Ядру — быстрый стакан, пока синхронизирован. Потеря синхронизации → сразу последний снимок books5 из памяти, если он
  не старше `venue.book_stale_ms` и его ts не меньше ts уже отданного ядру; иначе пара недостоверна (как раньше).
  Снимок books5, пришедший, пока быстрый не синхронизирован, — сразу ядру (в том числе на старте). Новый снимок books
  после пересподписки → снова быстрый.
- Переподписка пары сторожем — тоже переход на снимок (стакан в памяти не сбрасывается, если снимка нет — как раньше).
- Обрыв страховочного соединения — недостоверны только пары, по снимкам которых ядро считало.
- Сторож пар: `backup-N` зарегистрированы без пар (живость видна, команд по парам нет). Подтверждение тихой пары по REST
  (book_print.rs), пока ядро на снимке, сравнивает первые 5 уровней.
- Сверки books со снимком books5 нет: номера каналов не связаны документацией, равенство ts как «одна книга» — не факт.
- Счёт переключений — журнал `venue.book_backup` не чаще `venue.book_backup_log_s` (новая настройка, только журнал).

## Риски

- Число соединений OKX удваивается — проверить на живом (коды закрытия 4008/4009).
- Тихие по верхним 5 уровням пары: последний снимок books5 может быть старше отданного по ts → при потере синхронизации
  страховки нет (вопрос Тиграну в docs/TASKS.md).
- Код не собирался локально (только CI).
