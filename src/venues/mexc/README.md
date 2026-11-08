# venues/mexc — адаптер MEXC (спот, API v3)

Реализует контракт `gateway/contract.rs` (решение 107). Всё — только по официальной документации MEXC; разбор
документации и сверка с багами BingX — `docs/notes/mexc-analysis.md`.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://www.mexc.com/api-docs/spot-v3/introduction | Основная документация: подпись, лимиты, эндпоинты, коды ошибок |
| https://github.com/mexcdevelop/websocket-proto | Схемы protobuf WebSocket (`PushDataV3ApiWrapper`, `PublicLimitDepthsV3Api`, `PrivateAccountV3Api`) |
| https://github.com/mexcdevelop/apidocs | Исходник документации v3 (сверка расхождений) |

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки площадки, секреты, HTTP, журнал, сырые сообщения; символ пары `CAKEUSDT` |
| `delist.rs` | Решение 226: делистинги — пары с venue.quote_asset и признаком риска в GET /api/v3/exchangeInfo: `st` = true (метка ST — зона риска делистинга; живой случай ASS 08.11), `status` 2 / 3, `tradeSideType` 3 / 4; записи без времени — монета на MEXC не торгуется. /api/v3/symbol/offline (только снятые) и /api/v3/announcements (без тикеров) не годятся |
| `rest.rs` | REST: подпись HMAC SHA256 по строке запроса, ключ в `X-MEXC-APIKEY`, все параметры в строке запроса; recvWindow < 60000 |
| `reply.rs` | Разбор ответа: 418/429 → `RateLimited` по статусу, 403 → WAF, `code` ≠ 0/200 — ошибка (и без `msg`); пауза по `Retry-After` (решение 118) |
| `errors.rs` | Коды ошибок MEXC → единые коды шлюза (429 → лимит; 700003/10073 → «часы»; 30035/60005 → права; 11444 у ордера/вывода → отказ) |
| `num.rs` | Числа из ответов без потери точности |
| `symbols.rs` | Справочник правил пар в памяти: шаги по решению 107, минимумы, статус, покупка/продажа через API, точность вывода |
| `symbols_parse.rs` | Разбор exchangeInfo и selfSymbols (в том числе `orderTypes` пары) |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs` | Снимок пар, фоновое обновление справочника, подписка (как у BingX) |
| `fees.rs` | Ставки комиссии по паре (`GET /api/v3/tradeFee`, вес 20) |
| `wallet.rs` | Монеты и сети (`config/getall`) с кэшем `venue.networks_fresh_ms`, комиссия вывода |
| `networks_parse.rs` | Разбор `config/getall`: код `netWork` ↔ имя `network`, адрес контракта, пределы, шаг вывода `withdrawIntegerMultiple` (только дробный) |
| `deposit_addr.rs` | Адрес депозита: кэш по (монета, сеть) на время процесса, адрес с `memo` — ошибка «не поддерживается» |
| `transfers.rs` | Вывод (`POST /api/v3/capital/withdraw`, `withdrawOrderId` с меткой попытки), его статус, поиск по `withdrawOrderId` при неизвестном итоге |
| `deposits.rs` | Статус депозита по хешу (`gateway::tx_hash::same`) |
| `deposit_hold.rs` | Удержания вывода (решение 150): предварительно зачисленные депозиты — `GET /api/v3/capital/deposit/hisrec` (вес 10), записи `status` = 9 PRE_SUCCESS за окно API по умолчанию (7 дней); торговать ими можно, выводить нельзя. Решение 166: отказ вывода 16021 → WithdrawLocked (errors.rs, по живому ответу 04.11) |
| `account.rs` | Балансы, «Проверить подключение» (`apiKeyInfo`: права, включая SPOT_DEAL_READ, IP, срок ключа) |
| `all_balances.rs` | Панель «Балансы» (решение 144): счета «Спот» (`GET /api/v3/account`) и «Фьючерсы» (`GET /api/v1/private/account/assets`) параллельно, ошибка счёта — в его `error`; других счетов (маржа, Earn, фонд) в API MEXC нет |
| `futures_rest.rs` | Подписанный GET фьючерсного API (свой домен `venue.futures_rest_url`, заголовки `ApiKey`/`Request-Time`/`Signature`, подпись `accessKey+timestamp+параметры`), коды ошибок фьючерсов, своя пауза по Retry-After |
| `trade.rs` | Ордер с худшей ценой; правила и пробный ордер — `order_rules.rs` |
| `order_rules.rs` | Правила ордера — одни у настоящего ордера и у проверки без исполнения (`check_order`, решение 134): API-сторона, тип `venue.order_type` (orderTypes; IOC/FOK при LIMIT), шаги, минимумы и максимум суммы, сроки итога и myTrades; пробный ордер `POST /api/v3/order/test` — `venue.order_test_first` |
| `order_state.rs` | Ожидание итога (сбой запроса — повтор), отмена по сроку, чтение итога после отмены; поиск и отмена по своему ID (`origClientOrderId`) |
| `fills.rs` | Комиссия по всем сделкам ордера: `myTrades` с `limit` (`venue.my_trades_limit`), сверка суммы `qty` с `executedQty`; разные `commissionAsset` — сумма по монете (решение 219) |
| `proto.rs` | Нужные сообщения protobuf (prost) |
| `depth.rs`, `market.rs`, `ws.rs`, `ws_conn.rs`, `book.rs` | WebSocket стаканов: частичный стакан `spot@public.limit.depth.v3.api.pb@SYM@N`, PING, стаканы в памяти, котировка; свежесть — по соединению и по кадру пары (`venue.book_stale_ms`) |
| `user_key.rs`, `user_stream.rs`, `user_stream_conn.rs`, `account_update.rs` | Поток баланса по listenKey (`spot@private.account.v3.api.pb`); после обрыва — тот же ключ, пока продление проходит |
| `tap.rs`, `feed.rs`, `tap_snap.rs` | Запись окна (решение 125): двоичные кадры стакана монеты из ws_conn.rs как есть; свои соединения на монету — лента `spot@public.aggre.deals.v3.api.pb@(10ms\|100ms)@<SYMBOL>` (venue.trades_interval) и изменения всей глубины `spot@public.aggre.depth.v3.api.pb@(10ms\|100ms)@<SYMBOL>` (venue.window_depth_interval) со снимком REST /api/v3/depth (venue.window_depth_limit) после первого кадра и при разрыве версий; отписка UNSUBSCRIPTION; разбор кадров (тела 303, 313, 314, снимок) |

## Что важно знать

- WebSocket: не больше 30 подписок на соединение (`venue.ws_symbols_per_conn` ≤ 30); кадры данных — двоичные protobuf,
  ответы — JSON; свой `{"method":"PING"}` раз в `venue.ws_ping_ms`.
- Лимиты: IP 300 веса / 10 с, UID 500 / 10 с (независимые корзины), ордера и отмены 12/с. `tradeFee` весит 20 **из корзины IP**
  (решение 118, А-🟠1): 1 запрос/с — уже 200 из 300; в той же корзине опрос ордера (2), история депозитов (10), getall (10),
  exchangeInfo (25), account (10), myTrades (10). `venue.fee_requests_per_sec` держать низким, `venue.fee_cache_s` — большим.
- HTTP 418/429 — всегда `RateLimited`; с заголовком `Retry-After` адаптер сам не шлёт запросов до его истечения (`reply.rs`).
- Фондового счёта нет: депозит на споте, `deposit_to_trading` ничего не делает.
- Минимального депозита в API нет — `venue.deposit_min_absent_ok` (решение 107).
- В живом `orderTypes` нет `IMMEDIATE_OR_CANCEL` (запись 2026-11-02) — проверяется пробным ордером, иначе `LIMIT`.
- Функции контракта только для DEX отвечают `NotApplicable`.
- Решение 118 (обязательные у MEXC): `venue.my_trades_limit` (1..1000, советуется 1000) — без неё ордер не ставится;
  `venue.book_stale_ms` — без неё котировок нет. Необязательная `venue.networks_fresh_ms` — кэш `config/getall`.
- `withdrawIntegerMultiple`: в таблице документации — «Withdraw amount increment», в примере — `"18"`. Шагом вывода
  считается только дробное значение (0 < v < 1); целое — в журнал, не применяется (сверить на живом ответе).
