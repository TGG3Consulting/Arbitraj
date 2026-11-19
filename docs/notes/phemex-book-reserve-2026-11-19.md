# Phemex: страховка стакана в памяти (решение 427, задача t211)

## Исходное (факт кода до 427)

- Стакан Phemex — уже поток изменений: `orderbook.subscribe [symbol]`, «depth = 30 and interval = 20ms»; локальный
  стакан по sequence (local_book.rs, depth.rs).
- Разрыв sequence — пара недостоверна до контрольного снимка («Snapshot messages are published with 60-second interval
  for client self-verification»), то есть до 60 с без стакана. Отписки одной пары нет: «It unsubscribes all orderbook
  related subscriptions».

## Варианты по официальной документации (phemex-docs.github.io; исходник source/includes/_spot.md, _generic.md, _contract.md)

| Вариант | Цитаты | Вывод |
|---|---|---|
| REST GET /md/orderbook по требованию | «Rest API has a request capacity in one minute window on user basis»; группа Others «100/minute»; «IP 5,000 requests in 5 minutes»; ответ — `type: snapshot`, `sequence` «current message sequence» | Не страховка: один застывший снимок, продолжать его изменениями потока нельзя (связь sequence REST и кадров не описана, решение 213); лимит делят балансы, вывод, сети, сторож 213, снимок ордера 238. Уже есть снимок REST для ордера при устаревшем стакане (решение 238). |
| Второй поток пары на отдельном соединении | «Each Client has concurrent connection limit to 5 in maximum», «Each connection has subscription limit to 20 in maximum», «Each connection has throttle limit to 20 request/s», «wss://ws.phemex.com 200/5m per IP» | **Выбран.** Запрета на одну пару в двух соединениях нет; обрыв основного соединения не задевает запасной. Цена — вдвое меньше пар: (5 − поток аккаунта) / 2 × 20 = 40 вместо 80. |
| Две подписки одной пары на одном соединении ([symbol] и [symbol, true]; кадры различимы по `depth`: «30 by default, 0 denotes fullbook») | «Subscribe full orderBook … full depth and interval = 100ms» | Не выбран: можно ли подписать одну пару дважды на одном соединении — не описано; обрыв соединения снимает оба потока; ёмкость та же (10 пар × 2 подписки). |

Запасной поток — настройкой venue.book_reserve: `depth30` (та же скорость 20 мс) или `full` (вся глубина, 100 мс).

## Как работает

- market-N (основной) и market-N-reserve (запасной) — одна пачка пар; у каждого свой локальный стакан.
- book_pick.rs: ядру — основной, пока синхронизирован. Потеря основного (разрыв, кадр не разобран, отказ подписки,
  обрыв) — сразу последний стакан запасного, если он новее отданного; не новее — стакан в памяти остаётся (он новейший
  верный), живость — по запасному соединению. После снимка основной снова главный.
- Никогда не старее отданного: порядок по `timestamp` кадра (нс). sequence двух соединений не сравниваются.
- Сверка: контрольный снимок с тем же sequence, что последнее применённое изменение, сравнивается с верхом стакана в
  памяти (local_book.rs Applied::Differs) — счёт и журнал.
- Журнал `venue.book_reserve`: 1-е, 2-е, 4-е… событие каждого вида, в тексте — всего с запуска.
- Сторож 202/213: запасное соединение без пар — лишних запросов REST нет.

## Не проверено на живом (docs/TASKS.md)

Принимает ли Phemex одну пару на двух соединениях; общие ли sequence / timestamp двух соединений; совпадает ли sequence
контрольного снимка с последним изменением. Тесты — на «Message sample» документации (tests/phemex_book_pick_427.rs).
