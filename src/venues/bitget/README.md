# venues/bitget — адаптер Bitget (спот, Classic API v2)

Реализует контракт `gateway/contract.rs` (решение 162). Всё — только по официальной документации Bitget; чек-лист
уроков (30 пунктов) и как каждый закрыт — `docs/notes/bitget-integration-2026-11-05.md`; анализ до кода —
`docs/notes/bitget-analysis-2026-11-05.md`.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://www.bitget.com/docs/classic/catalog | Каталог Classic v2 (старые адреса `/api-doc/...` молча ведут на обзор UTA) |
| https://www.bitget.com/docs/classic/rest-api | Подпись, заголовки ACCESS-*, HTTP-коды, лимиты (429, «6000/IP/Min»), права ключа |
| https://www.bitget.com/docs/catalog/classic-spot-market/classic-spot-market | Get Coin Info, Get Symbol Info, OrderBook |
| https://www.bitget.com/docs/catalog/classic-spot-trade/classic-spot-trade | Place Order, Cancel Order, Get Order Info (feeDetail), Fills |
| https://www.bitget.com/docs/catalog/classic-spot-account/classic-spot-account | Assets, Info (authorities), Upgrade Status, BGB Deduct, Transfer, Deposit Address/Records, Withdraw, Withdrawal Records, Modify Deposit Account |
| https://www.bitget.com/docs/catalog/classic-common-public/classic-common | Trade Rate, All Trade Rate, Server Time |
| https://www.bitget.com/docs/catalog/classic-common-account/classic-common-account | Funding Assets, All Account Balance, Bot Assets |
| https://www.bitget.com/docs/classic/websocket/intro | Адреса WS, вход, ping/pong, лимиты подписок |
| https://www.bitget.com/docs/classic/websocket/spot/public/Depth-Channel | books1 / books5 / books15 / books, seq |
| https://www.bitget.com/legacy-docs/classic/best-practices | Правило clientOid, жизненный цикл IOC |
| https://www.bitget.com/docs/api/error-code | Коды ошибок — «Demo error codes», только 7 кодов |
| https://github.com/BitgetLimited/v3-bitget-api-sdk | Официальный SDK: `bitget-python-sdk-api/bitget/utils.py` (подпись — вектор теста), `ws/bitget_ws_client.py` |

Версия API: **Classic v2** (классический счёт). UTA (`/api/v3`) — без справочника монет с сетями; счёт в режиме UTA
«Проверить подключение» отвергает (upgrade-status = success).

## Счета Bitget

- **spot** — торговля и баланс бота (`/api/v2/spot/account/assets`).
- **p2p** («P2P/funding account») — фонд. На какой счёт зачисляется депозит и с какого идёт вывод, в документации Classic
  **не сказано**. Варианты: в кабинете (или `POST /api/v2/spot/wallet/modify-deposit-account`) назначить зачисление
  монеты на spot; либо `venue.deposit_to_spot` = да (после зачисления p2p → spot) и, если вывод только с фонда,
  `venue.withdraw_from_trade` = да (перед выводом spot → p2p). Перевод — `POST /api/v2/spot/wallet/transfer`, право wtow.
- Вывод — только на адреса из адресной книги кабинета (код 11000) и только с IP из белого списка ключа.

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки, секреты (ключ, секрет, passphrase), HTTP, журнал, сырые сообщения |
| `sign.rs` | Подпись: Base64(HMAC-SHA256(timestamp+METHOD+путь?запрос+тело)), заголовки ACCESS-* |
| `rest.rs` | REST: параметры по имени без кодирования (как SDK), POST — тело JSON, locale; резерв только при сбое сети; решение 201 — ручная работа ждёт конец паузы 429 эндпоинта |
| `resolve.rs` | Решение 231: исход операции «неизвестен» по «чем найти» из ошибки — ордер по своему ID / ID ордера (те же запрос, отмена и доведение, что у постановки); вывод — по своему ID в истории выводов |
| `rest_book.rs` | Решение 201: снимок стакана REST GET /api/v2/spot/market/orderbook для котировки исполнения при тихом стакане потока |
| `reply.rs` | Разбор {code "00000", msg, data}; 429 — пауза всех запросов на Retry-After или venue.rate_limit_pause_ms |
| `errors.rs` | Коды Bitget и HTTP-статусы → единые коды (429, 401, 403, 5XX, 11000, остальное — отказ по операции) |
| `num.rs` | Числа и флаги-строки ("true"/"false") без потери точности |
| `symbols.rs`, `symbols_parse.rs` | Справочник пар: status, шаги из pricePrecision / quantityPrecision, minTradeUSDT |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs`, `market.rs` | Снимок пар и SpotPairs, фоновое обновление справочника, подписка (решение 131) |
| `fees.rs` | Ставки ключа all-trade-rate одним запросом; скидка BGB (deduct-info) — расчёт по обычной ставке |
| `wallet.rs`, `networks_parse.rs` | Монеты и сети Get Coin Info (весь — вне рабочих потоков, за Arc; одна монета ?coin=), комиссия вывода |
| `deposit_addr.rs` | Адрес депозита (кэш; tag — не поддерживается) |
| `records.rs`, `deposits.rs`, `transfers.rs` | Окно истории venue.history_window_s; статус депозита по хешу; вывод со своим clientOid, статус и поиск |
| `fund_move.rs` | Перевод spot ↔ p2p по настройкам |
| `account.rs` | Балансы спота (все и одной монеты), «Проверить подключение» (authorities, IP, Classic/UTA, BGB) |
| `all_balances.rs` | Панель «Балансы»: Спот, Фонд, Боты (спот, фьючерсы), итог всех счетов в USDT |
| `order_rules.rs` | Правила ордера — одни у ордера и у `check_order` (решение 134); пробного ордера нет |
| `trade.rs`, `order_state.rs`, `order_fee.rs` | Ордер limit + ioc/fok, итог по orderInfo, отмена остатка, поиск по clientOid, комиссия из feeDetail — по монетам: BGB и остаток в получаемой монете (решение 219) |
| `depth.rs`, `ws.rs`, `ws_conn.rs`, `book.rs` | Стаканы books1/5/15 снимками, отсев по seq (решение 109), ping, котировка |
| `book_decided.rs` | Решение 217: снимок стакана для `quote_snapshot` — тот же, по которому котировали (поток или снимок REST котировки исполнения у пары без потока, решение 201; не старше feed.stale_after_ms), без запросов |

## Чего нет и почему (вопросы Тиграну)

- **Поток аккаунта (баланс по WS, решение 76)** — страница приватного канала баланса спота Classic не читается
  (переадресация на UTA), а время входа противоречиво (текст — мс, пример Java и SDK — секунды). Зачисление — по
  `deposit_status` (REST). `venue.balance_events` у Bitget не действует.
- **Окна (решение 125, window_tap)** — не подключены (контракт отвечает NotApplicable); канал сделок и снимки — при
  следующем шаге.
- **Удержание вывода (решения 150, 166)** — статуса «предварительно зачислено» и «сколько можно вывести» в документации нет → `withdraw_hold` отвечает «неизвестно» (умолчание контракта; проверка не блокирует, страхует повтор вывода). Кода «заблокировано для вывода» в «Demo error codes» нет — WithdrawLocked не выдаётся.
- **Скидка BGB** — размер скидки не описан: расчёт всегда по обычной ставке (худший случай), факт — из feeDetail.
- **buyLimitPriceRatio / sellLimitPriceRatio** — от какой цены считать, не определено: не проверяются, отказ даст биржа.
- **extraWithdrawFee ≠ 0** — как считать долю (из суммы или сверху), не описано: комиссия неизвестна, вывода нет.

## Что важно знать

- WebSocket: адрес — `venue.ws_url`; «ping» строкой раз в `venue.ws_ping_ms` (< 2 мин), тишина `venue.ws_silence_ms`;
  ≤ 10 сообщений в секунду, запрос подписки ≤ 4096 байт, ≤ 1000 каналов на соединение (совет — < 50), 240 подписок в
  час на соединение — подписка только при подключении.
- Лимиты REST: свои на каждом эндпоинте («N times/1s» по UID или IP), общий 6000/IP/мин; Retry-After не описан.
- Функции контракта только для DEX отвечают `NotApplicable`.
