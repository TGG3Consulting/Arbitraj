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
| `delist.rs` | Решение 226: делистинги — пары с venue.quote_asset и признаком риска в GET /api/v3/exchangeInfo: `status` 2 / 3, `tradeSideType` 3 / 4 — запись без времени, монета на MEXC не торгуется; `st` = true (метка ST; живой случай ASS 08.11) — запись с признаком st, блок только при delist.st_block = да (уточнение 08.11). /api/v3/symbol/offline (только снятые) и /api/v3/announcements (спотовых делистингов за 19 суток нет, «ST Warning» = метка st) не используются |
| `rest.rs` | REST: подпись HMAC SHA256 по строке запроса, ключ в `X-MEXC-APIKEY`, все параметры в строке запроса; recvWindow < 60000 |
| `resolve.rs` | Решение 231: исход операции «неизвестен» по «чем найти» из ошибки — ордер по своему ID / ID ордера (те же запрос, отмена и доведение, что у постановки); вывод — по своему ID в истории выводов |
| `ip_weight.rs` | Тигран 09.11 (согласовано): свой ограничитель веса публичных REST-запросов — скользящее окно 10 с документации, сумма веса (depth 3, exchangeInfo 25) не больше `venue.public_ip_weight_10s`; путь сделки (Trade, котировка для исполнения, терпение ручной работы) не ждёт, но учитывается; фон ждёт места. Не задана — ограничителя нет |
| `reply.rs` | Разбор ответа: 418/429 → `RateLimited` по статусу, 403 → WAF, `code` ≠ 0/200 — ошибка (и без `msg`); пауза по `Retry-After` (решение 118) |
| `errors.rs` | Коды ошибок MEXC → единые коды шлюза (429 → лимит; 700003/10073 → «часы»; 30035/60005 → права; 11444 у ордера/вывода → отказ). Отказы вывода (решение 253, дополнение t82): 10212 → InvalidAddress; 157005/10265/10098/730100 → AccountRestricted; дополнение t85: у вывода 60005 → AccountRestricted; решение 263: HTTP 5XX с кодом не из списка (`from_code_http`) — у ордера, вывода, перевода «итог неизвестен», а не отказ |
| `errors_fail.rs` | Сбои HTTP-обмена (`from_fail`, `fail_for`) — вынесены из `errors.rs` без изменения логики (решение 263); снаружи — `errors::from_fail` / `errors::fail_for` |
| `num.rs` | Числа из ответов без потери точности |
| `symbols.rs` | Справочник правил пар в памяти: шаги по решению 107, минимумы, статус, покупка/продажа через API, точность вывода |
| `symbols_parse.rs` | Разбор exchangeInfo и selfSymbols (в том числе `orderTypes` пары) |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs` | Снимок пар, фоновое обновление справочника, подписка (как у BingX); пара, где тип venue.order_type недопустим по orderTypes (решение 133), — признак `order_block` с причиной, ядро её не считает (решение 254) |
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
| `market_sell.rs` | Решение 268: рыночная продажа (контракт `trade_market_sell`) — ордер типа «market» на количество базовой монеты после отказа лимитного ордера «цена вне диапазона»; правила пары и разбор итога — те же, что у лимитного (`order_rules.rs`, `trade.rs`); код PriceOutOfRange рыночный ордер не отдаёт |
| `order_rules.rs` | Правила ордера — одни у настоящего ордера и у проверки без исполнения (`check_order`, решение 134): API-сторона, тип `venue.order_type` (orderTypes; IOC/FOK при LIMIT), шаги, минимумы и максимум суммы, сроки итога и myTrades; пробный ордер `POST /api/v3/order/test` — `venue.order_test_first`; `order_block` — признак пары «не примет наш ордер» для справочника ядра (решение 254) |
| `order_state.rs` | Ожидание итога (сбой запроса — повтор), отмена по сроку, чтение итога после отмены; поиск и отмена по своему ID (`origClientOrderId`) |
| `fills.rs` | Комиссия по всем сделкам ордера: `myTrades` с `limit` (`venue.my_trades_limit`), сверка суммы `qty` с `executedQty`; разные `commissionAsset` — сумма по монете (решение 219) |
| `proto.rs` | Нужные сообщения protobuf (prost) |
| `depth.rs`, `market.rs`, `ws.rs`, `ws_conn.rs`, `book.rs` | WebSocket стаканов: частичный стакан `spot@public.limit.depth.v3.api.pb@SYM@N`, PING, стаканы в памяти, котировка; свежесть — по соединению и по кадру пары (`venue.book_stale_ms`); решение 202 — учёт свежести для сторожа ядра (gateway/feed_reg.rs), переподписка одной пары (`UNSUBSCRIPTION` + `SUBSCRIPTION`) и переподключение одного соединения по команде сторожа |
| `book_print.rs` | Решение 213 по содержимому (Тигран 08.11): `GET /api/v3/depth` (limit = `venue.depth_levels`) — первые N уровней ответа и стакана в памяти для подтверждения тихой пары ядром (core/feed_print.rs); Решение 238: тот же запрос — снимок стакана для ордера (`snapshot`, контракт `quote_rest`), все уровни ответа |
| `book_rest.rs` | t79 (дополнение 10.11 к 202/213, Тигран 10.11: согласовано): ответ REST сторожа (GET /api/v3/depth, первые `venue.depth_levels` уровней) стаканом тихой пары «книга не менялась с переподписки» (контракт `book_from_rest`): время кадра пары — получение ответа, подтверждения нет (свежий `venue.book_stale_ms`), кадр пары после отправки запроса — не ставится, стакана нет — только у пары, подписанной в живой сессии (`bind` из ws_conn.rs); следующий кадр заменяет |
| `user_key.rs`, `user_stream.rs`, `user_stream_conn.rs`, `account_update.rs` | Поток баланса по listenKey (`spot@private.account.v3.api.pb`); после обрыва — тот же ключ, пока продление проходит |
| `tap.rs`, `feed.rs`, `tap_snap.rs` | Запись окна (решение 125): двоичные кадры стакана монеты из ws_conn.rs как есть; свои соединения на монету — лента `spot@public.aggre.deals.v3.api.pb@(10ms\|100ms)@<SYMBOL>` (venue.trades_interval) и изменения всей глубины `spot@public.aggre.depth.v3.api.pb@(10ms\|100ms)@<SYMBOL>` (venue.window_depth_interval) со снимком REST /api/v3/depth (venue.window_depth_limit) после первого кадра и при разрыве версий; отписка UNSUBSCRIPTION; разбор кадров (тела 303, 313, 314, снимок) |

## Что важно знать

- WebSocket: не больше 30 подписок на соединение (`venue.ws_symbols_per_conn` ≤ 30); кадры данных — двоичные protobuf,
  ответы — JSON; свой `{"method":"PING"}` раз в `venue.ws_ping_ms`.
- Лимиты: IP 300 веса / 10 с, UID 500 / 10 с (независимые корзины), ордера и отмены 12/с. `tradeFee` весит 20 **из корзины IP**
  (решение 118, А-🟠1): 1 запрос/с — уже 200 из 300; в той же корзине опрос ордера (2), история депозитов (10), getall (10),
  exchangeInfo (25), account (10), myTrades (10). `venue.fee_requests_per_sec` держать низким, `venue.fee_cache_s` — большим.
- HTTP 418/429 — всегда `RateLimited`; с заголовком `Retry-After` адаптер сам не шлёт запросов до его истечения (`reply.rs`).
- Свой ограничитель публичных запросов (Тигран 09.11): `venue.public_ip_weight_10s` — доля корзины IP 300 / 10 с для
  depth и exchangeInfo (`ip_weight.rs`); подписанные запросы с весом по IP он не считает.
- Фондового счёта нет: депозит на споте, `deposit_to_trading` ничего не делает.
- Минимального депозита в API нет — `venue.deposit_min_absent_ok` (решение 107).
- В живом `orderTypes` нет `IMMEDIATE_OR_CANCEL` (запись 2026-11-02) — проверяется пробным ордером, иначе `LIMIT`.
- Функции контракта только для DEX отвечают `NotApplicable`.
- Решение 118 (обязательные у MEXC): `venue.my_trades_limit` (1..1000, советуется 1000) — без неё ордер не ставится;
  `venue.book_stale_ms` — без неё котировок нет. Необязательная `venue.networks_fresh_ms` — кэш `config/getall`.
- `withdrawIntegerMultiple`: в таблице документации — «Withdraw amount increment», в примере — `"18"`. Шагом вывода
  считается только дробное значение (0 < v < 1); целое — в журнал, не применяется (сверить на живом ответе).
- Сторож пар (решения 202/213, 08.11): `feed_health` — scheduled = false, per_pair = true, **confirm = true — по
  содержимому** (Тигран 08.11, `book_print.rs`): номер не сравнивается — у `version` кадра частичного стакана в
  документации только «Version number», связи с `lastUpdateId` ответа `GET /api/v3/depth` нет (она описана только для
  `fromVersion`/`toVersion` канала изменений `spot@public.aggre.depth`); сравниваются первые `venue.depth_levels`
  уровней ответа `GET /api/v3/depth` (limit = `venue.depth_levels`, вес 3) и стакана в памяти: равны — тихая пара
  подтверждена (`book_confirmed`, book.rs), нет — переподписка пары. Темп — `venue.quiet_confirm_*` (совет — FIRST_RUN).
