# venues/binance — адаптер Binance (спот API + Wallet SAPI, решение 160)

Реализует контракт `gateway/contract.rs` целиком. Всё — только по официальной документации Binance; разбор перед
подключением — `docs/notes/binance-analysis-2026-11-05.md`. Живых ответов на момент написания нет (из среды разработки
`api.binance.com` закрыт прокси): всё, что помечено «проверить на живом», сверяется по записям (tests/README.md, «Binance»).

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://developers.binance.com/legacy-docs/binance-spot-api-docs/rest-api | REST: адреса, LIMITS (429/418, Retry-After), exchangeInfo, depth, order, order/test, account, commission, myTrades |
| …/rest-api/request-security | Подпись HMAC (пример с секретом `NhqPtmd…` → `c8db5682…`) |
| …/web-socket-streams | Комбинированный поток, Partial Book Depth, Diff Depth, Trade, WebSocket Limits, ping/pong, SUBSCRIBE и коды ошибок |
| …/websocket-api/request-security, …/websocket-api/user-data-stream-requests | Подпись WebSocket API (параметры по алфавиту), userDataStream.subscribe.signature |
| …/user-data-stream | События аккаунта: balanceUpdate, outboundAccountPosition, executionReport, eventStreamTerminated |
| …/filters, …/enums, …/errors, …/faqs/commission_faq | Фильтры пар, статусы, коды ошибок, расчёт комиссии и скидки BNB |
| https://developers.binance.com/docs/wallet/ (capital, account, asset), официальный SDK binance-connector-rust | getall, withdraw/apply, withdraw/history, deposit/address, deposit/hisrec, apiRestrictions, get-funding-asset, asset/transfer, wallet/balance |

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки, секреты `binance.api_key` / `binance.api_secret`, HTTP, журнал; символ `CAKEUSDT` |
| `sign.rs` | Подпись HMAC SHA256 hex: строка REST (порядок отправки), строка WebSocket API (по алфавиту) |
| `rest.rs`, `reply.rs`, `errors.rs` | REST: X-MBX-APIKEY, recvWindow ≤ 60000, резервный адрес только при сбое сети; 418/429/−1003 — пауза по Retry-After или `venue.rate_limit_pause_ms` отдельно для /api и /sapi; коды → единые |
| `num.rs` | Числа без потери точности |
| `symbols.rs`, `symbols_parse.rs` | exchangeInfo (вне рабочих потоков): tickSize, stepSize, PRICE_FILTER, LOT_SIZE, NOTIONAL/MIN_NOTIONAL, статус, isSpotTradingAllowed, permissionSets ∩ права аккаунта |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs` | Снимок пар (SpotPairs), фоновое обновление, подписка (как MEXC); с первой подпиской — поток аккаунта и круг остатка BNB |
| `order_rules.rs` | ОДНА функция правил ордера (решение 134): у ордера и у `check_order`; LIMIT + IOC/FOK, округление, пределы, пробный `POST /api/v3/order/test` |
| `trade.rs`, `order_state.rs`, `fills.rs` | Ордер с ответом FULL (итог сразу), иначе опрос/отмена; поиск по `origClientOrderId` при неизвестном итоге; комиссия — fills ответа или myTrades со сверкой с executedQty |
| `fee_rates.rs`, `fees.rs`, `discount_balance.rs` | Ставки `account/commission` (стандартная + налог + специальная; taker + buyer/seller) фоном; скидка BNB по `venue.fee_deduction` и остатку BNB (решение 158) |
| `book.rs`, `depth.rs`, `market.rs`, `ws.rs`, `ws_conn.rs` | Стаканы `<symbol>@depth<N>[@100ms]` комбинированным потоком; одинаковый lastUpdateId — без разбора и без отдачи ядру (решение 109); котировка |
| `wallet.rs`, `networks_parse.rs` | config/getall (вне рабочих потоков, кэш): сеть BSC, contractAddress, флаги, комиссия, пределы, шаг вывода withdrawIntegerMultiple, минимум депозита — решение человека |
| `delist.rs` | Решение 175: делистинги — GET /sapi/v1/spot/delist-schedule (MARKET_DATA, вес 100, подпись — по спецификации binance-api-swagger): `delistTime` (мс) и список символов; символ → монета по справочнику пар (symbols.rs). Разбор строго по спецификации (`delistTime`, `symbol`); живой ответ — записать и сверить |
| `deposit_addr.rs` | Адрес депозита, кэш (монета, сеть); адрес с tag — отказ |
| `transfers.rs` | Вывод withdraw/apply (свой withdrawOrderId, walletType), статус по idList, поиск по withdrawOrderId; не чаще `venue.withdraw_history_min_ms` |
| `deposits.rs` | Депозит по хешу (txId), удержание вывода — статус 6 (решение 150); отказ вывода −4060 CAPITAL_WITHDRAW_USER_ASSET_LOCK_DEPOSIT → WithdrawLocked (решение 166, errors.rs) |
| `fund_move.rs` | Funding → Spot после зачисления (`venue.deposit_to_spot`) |
| `account.rs`, `all_balances.rs`, `key_check.rs` | Баланс спота; панель «Балансы» (Спот, Фонд, Все кошельки); «Проверить подключение» (apiRestrictions, canTrade/…, travel rule) |
| `user_stream.rs`, `user_stream_conn.rs`, `account_update.rs` | Поток аккаунта WebSocket API (`userDataStream.subscribe.signature`), balanceUpdate → MarketEvent::Balance |
| `tap.rs`, `tap_decode.rs`, `feed.rs` | Окна (решение 125): кадры стакана, `<symbol>@trade`, `<symbol>@depth` + снимок `/api/v3/depth` при разрыве U/u |

## Что важно знать

- Лимиты `/api/*` — по IP (REQUEST_WEIGHT из rateLimits живого exchangeInfo); после 429 продолжать нельзя — бан 418.
  `/sapi/*` — свой предел на эндпоинт; `withdraw/history` весит 18000 (UID) — ~10 запросов в минуту на аккаунт.
- WebSocket: 5 входящих сообщений/с на соединение (включая pong) — `venue.ws_subs_per_sec` < 5; до 1024 потоков; 300
  подключений за 5 минут с IP. Свой ping не нужен: сервер шлёт ping раз в 20 с, бот отвечает pong.
- Депозит идёт на Спот или Фонд — по выбору в кабинете; бот торгует на споте (Funding → Spot — `venue.deposit_to_spot`).
- Удержание вывода: депозит статуса 6 — торговать можно, выводить нельзя (ошибка вывода −4060).
- Белый список адресов вывода (−4035) и задержка нового адреса (−4036): адрес кошелька DEX добавить заранее.

## Проверить на живых ответах (вопросы — в отчёте решения 160)

1. `venue.bsc_network` = `BSC`, `contractAddress` у CAKE/USDT, что приходит у BNB (адреса нет — решение 62).
2. `depositDust` — минимум депозита? (`venue.deposit_dust_as_min`, решение Тиграна); `denomination` у «1M»-монет.
3. Статусы вывода 1 и 5 не описаны — сейчас BadResponse с записью (не угадываем).
4. Алфавит и длина `newClientOrderId` (ответ −1100 на пробном ордере с «#»); сейчас буквы, цифры, «_» до 32.
5. `permissionSets` ∩ `permissions` аккаунта — пара CAKEUSDT торгуется через API.
6. Множитель `discount` (0.75 или 0.25 в примерах документации); монета комиссии в fills при скидке BNB.
7. Ответ `questionnaire-requirements` (значение "NIL"), get-funding-asset — видна ли крипта на Фонде.
8. Формат событий потока аккаунта WebSocket API (обёртка `subscriptionId` + `event`).
