# Bybit: страховка стакана в памяти (решение 428, t212, 19.11.2026)

Источник — официальная документация Bybit V5 (исходник github.com/bybit-exchange/docs, коммит e3936788 от 19.11.2026,
и живые страницы bybit-exchange.github.io/docs/v5/...). Запись живых кадров — tests/data/cex_samples_2026_11_04/bybit_ws.jsonl
(только orderbook.50.CAKEUSDT).

## Что было

Основной поток — уже изменения: `orderbook.{venue.depth_levels}.{SYM}` (snapshot + delta, `u`). Разрыв `u` → стакан пары
недостоверен, unsubscribe + subscribe за новым снимком; второй разрыв раньше venue.ws_reconnect_pause_ms → переподключение
соединения. Всё это время (и при обрыве соединения, и при отказе подписки) стакана пары у ядра нет.

## Почему страховка — другая глубина той же пары на отдельном соединении

| Факт документации | Цитата | Раздел |
| --- | --- | --- |
| Разные глубины одной пары — штатно | «Subscribe to the orderbook stream. Supports different depths.» | Orderbook |
| Порядок кадров разных глубин сравним | seq: «Cross sequence. You can use this field to compare different levels orderbook data, and for the smaller seq, then it means the data is generated earlier» | Orderbook, Response Parameters |
| Частоты спота | «Level 1 data, push frequency: 10ms», «Level 50 … 20ms», «Level 200 … 100ms», «Level 1000 … 200ms» | Orderbook → Depths |
| Глубина 1 — без изменений | «Linear, inverse, spot level 1 data has snapshot message only» | Orderbook, info |
| Предел запроса подписки | «Spot can input up to 10 args for each subscription request sent to one connection» | Connect → Args limits |
| Предел длины args | «for one public connection, you cannot have length of "args" array over 21,000 characters» | Connect → Args limits |
| Подключения | «Do not build over 500 connections in 5 minutes. This is counted per WebSocket domain»; «Do not establish more than 1,000 connections per IP for market data» | Connect → IP Limits; Rate Limit → Websocket IP limit |
| Пересподписка без разрыва соединения | «You can dynamically subscribe and unsubscribe from topics without unsubscribing from the WebSocket» | Connect → Understanding WebSocket Filters |

Не описано и поэтому не используется: подписка той же темы (той же глубины) на двух соединениях — запрещено настройкой
(venue.book_backup_levels ≠ venue.depth_levels). Отдельное соединение выбрано, чтобы обрыв соединения основного потока не
уносил страховку. REST для страховки не нужен (снимок приходит после subscribe сам).

## Как работает (venues/bybit/backup.rs, backup_side.rs, ws_lane.rs, ws_give.rs)

- Основной синхронизирован (был снимок, разрыва нет) — ядру основной.
- Разрыв `u`, обрыв соединения, отказ подписки — сразу последний стакан страховки, если он моложе venue.book_stale_ms и его
  `seq` больше отданного; иначе — как раньше (стакан недостоверен) и первый кадр страховки новее отданного.
- Основной после разрыва ждёт новый снимок, изменения до него ядру не идут; после снимка — снова основной, как только его
  `seq` больше отданного (журнал «обратно на основной»).
- При смене источника ядру не отдаётся `seq` не больше отданного; внутри одного непрерывного потока порядок держит `u`
  (ревью 428: снимок после «u=1, restart of the service» не застревает; пока источник — страховка, идёт каждый её
  кадр). Отдача — под замком пары (два соединения не путают порядок).
- Сверка: при равном `seq` у основного и страховки — отпечаток первых K = min(глубин) уровней обеих сторон; расхождение →
  основной переподписывается (без сброса локального стакана — до снимка не отдаётся), ядру — страховка.
- Ядру — не больше venue.depth_levels уровней (у страховки глубины 1 — лучший уровень).
- Сторож пар (202/213): соединения backup-N не регистрируются; номер `u` для подтверждения «по номеру» (глубина 1000)
  передаётся, только когда стакан ядра — из основного. Окна (125) пишут кадры основного.
- Счётчики и журнал `venue.book_backup` (backup_log.rs): на страховку, обратно, пересинхронизаций, расхождений, потерь
  страховки, сверок — запись на 1-е, 2-е, 4-е… событие вида.

## Что не проверено (docs/TASKS.md)

Равный `seq` у двух глубин = одно и то же состояние книги — документация прямо не пишет (только «меньше — раньше»);
записи двух глубин одной пары нет. Если на живой записи равный seq не значит одно состояние — сверку убрать.
