# venues/bybit — адаптер Bybit (API V5, только спот)

Реализует контракт `gateway/contract.rs` (решение 136). Только `category=spot`: фьючерсов, маржи и опционов нет
(ордер — `isLeverage=0`; ключ с правами деривативов не проходит «Проверить подключение»). Чек-лист уроков BingX/MEXC и
как каждый закрыт — `docs/notes/bybit-integration-2026-11-04.md`.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://bybit-exchange.github.io/docs/v5/intro | Документация V5 (исходник — github.com/bybit-exchange/docs, коммит 76aa20a от 2026-11-03) |
| https://bybit-exchange.github.io/docs/v5/guide | Подпись (X-BAPI-*), ответ {retCode, retMsg, result}, адреса REST |
| https://bybit-exchange.github.io/docs/v5/rate-limit | 403 «access too frequent» (10 минут), 10006, заголовки X-Bapi-Limit*, лимиты эндпоинтов |
| https://bybit-exchange.github.io/docs/v5/error | Коды ошибок (UTA, Spot Trade, Asset) |
| https://bybit-exchange.github.io/docs/v5/ws/connect | WebSocket: адреса, auth, ping, пределы args (10 на запрос, 100 на соединение, 21000 символов) |
| https://bybit-exchange.github.io/docs/v5/acct-mode | Режимы аккаунта: classic недоступен, UTA 1.0 / 2.0 |
| github.com/bybit-exchange/pybit | Официальный SDK — сверка обработки snapshot/delta (непрерывность `u` SDK не проверяет) |

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки, секреты, HTTP, журнал, сырые сообщения, окна, состояние приватного потока; символ `CAKEUSDT` |
| `http.rs` | HTTP-обмен со сроками gateway/http.rs и заголовками X-Bapi-Limit* |
| `rest.rs` | Подпись HMAC SHA256 (`timestamp+key+recv_window+query|body`), GET/POST, резервный адрес только при сбое сети |
| `reply.rs` | Разбор ответа; пауза 10 мин после HTTP 403; пауза эндпоинта до X-Bapi-Limit-Reset-Timestamp (Status=0, 10006) |
| `errors.rs` | retCode / HTTP → единые коды; «итог неизвестен» у ордера, вывода, перевода |
| `num.rs` | Числа из строк и чисел JSON без потери точности |
| `symbols.rs`, `symbols_parse.rs` | Спот-пары instruments-info: basePrecision, tickSize, minOrderAmt, maxLimitOrderQty, status |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs` | Снимок пар (SpotPairs), фоновое обновление, подписка (как MEXC) |
| `fees.rs` | Ставки taker/maker по паре (`/v5/account/fee-rate`, 5/s) фоном |
| `market.rs`, `ws.rs`, `ws_conn.rs` | WebSocket стаканов `orderbook.{N}.{SYM}`: пачки ≤10 args, ≤100 на соединение, ping, переподключение |
| `depth.rs`, `local_book.rs` | Разбор кадров (serde со ссылками), локальный стакан snapshot+delta с проверкой `u`, пересинхронизация |
| `book.rs` | Стаканы в памяти, котировка на объём, свежесть соединения и пары |
| `wallet.rs`, `networks_parse.rs` | Монеты и сети (`/v5/asset/coin/query-info`), запрос одной монеты, стоимость вывода |
| `deposit_addr.rs` | Адрес депозита (master), tag — «не поддерживается» |
| `deposits.rs` | Статус депозита по txID |
| `transfers.rs` | Вывод (forceChain=1, feeType=1, requestId, accountType = venue.withdraw_account), статус, поиск при неизвестном итоге |
| `fund_move.rs` | FUND → UNIFIED после зачисления (venue.deposit_to_spot) |
| `account.rs` | Балансы UNIFIED, баланс одной монеты, «Проверить подключение» (права, master, UTA, IP) |
| `order_rules.rs` | Правила ордера — одни у ордера и check_order; предел цены price-limit (venue.price_limit_check) |
| `trade.rs`, `order_state.rs`, `fills.rs` | Limit + IOC/FOK, ожидание итога (поток или REST), отмена по сроку, комиссия и её монета |
| `user_stream.rs`, `user_stream_conn.rs`, `private_cache.rs` | Приватный поток order.spot, execution.spot, wallet (venue.private_stream) |
| `tap.rs`, `feed.rs` | Окна (решение 125): кадры стакана, publicTrade, стакан окна `orderbook.{venue.window_depth_limit}` |

## Что важно знать

- Счёт: торговля и балансы — UNIFIED; депозит по умолчанию приходит на FUND → `deposit_to_trading` переводит FUND →
  UNIFIED при `venue.deposit_to_spot = да` (или в кабинете Bybit назначить депозит на UNIFIED и не включать перевод).
- Пробного ордера у спота нет (`/v5/order/pre-check` — только inverse/linear/option): `check_order` — только правила.
- Признака «торгуется через API» у пары нет: покупка/продажа через API = статус `Trading`.
- Процентная комиссия вывода (`withdrawPercentageFee` ≠ 0) ядром не учитывается — комиссия вывода такой сети «неизвестна».
- Изменения баланса ядру (решение 76) не шлются: wallet описывает только UNIFIED и без причины; депозит — опросом статуса.
- Непрерывность `u` у спота в документации не обещана; проверяется рост `u`. На живой записи 04.11 `u` шёл строго +1.
