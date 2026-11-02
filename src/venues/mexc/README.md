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
| `rest.rs` | REST: подпись HMAC SHA256 по строке запроса, ключ в `X-MEXC-APIKEY`, все параметры в строке запроса |
| `errors.rs` | Коды ошибок MEXC → единые коды шлюза |
| `num.rs` | Числа из ответов без потери точности |
| `symbols.rs` | Правила пар (exchangeInfo + selfSymbols ключа): шаги по решению 107, минимумы, статус, покупка/продажа через API |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs` | Снимок пар, фоновое обновление справочника, подписка (как у BingX) |
| `fees.rs` | Ставки комиссии по паре (`GET /api/v3/tradeFee`, вес 20) |
| `wallet.rs` | Монеты и сети (`config/getall`): код `netWork` ↔ имя `network`, адрес контракта, комиссия вывода, адрес депозита |
| `transfers.rs` | Вывод (`POST /api/v3/capital/withdraw`), его статус, статус депозита |
| `account.rs` | Балансы, «Проверить подключение» (`apiKeyInfo`: права, IP, срок ключа) |
| `trade.rs` | Ордер с худшей ценой, тип — `venue.order_type`; пробный ордер перед настоящим — `venue.order_test_first` |
| `order_state.rs` | Ожидание итога, отмена остатка, комиссия по `myTrades`, поиск по своему ID (`origClientOrderId`) |
| `proto.rs` | Нужные сообщения protobuf (prost) |
| `depth.rs`, `market.rs`, `ws.rs`, `ws_conn.rs`, `book.rs` | WebSocket стаканов: частичный стакан `spot@public.limit.depth.v3.api.pb@SYM@N`, PING, стаканы в памяти, котировка |
| `user_key.rs`, `user_stream.rs`, `user_stream_conn.rs`, `account_update.rs` | Поток баланса по listenKey (`spot@private.account.v3.api.pb`) |

## Что важно знать

- WebSocket: не больше 30 подписок на соединение (`venue.ws_symbols_per_conn` ≤ 30); кадры данных — двоичные protobuf,
  ответы — JSON; свой `{"method":"PING"}` раз в `venue.ws_ping_ms`.
- Лимиты: IP 300 веса / 10 с, UID 500 / 10 с, ордера и отмены 12/с. `tradeFee` весит 20 — `venue.fee_requests_per_sec` = 1.
- Фондового счёта нет: депозит на споте, `deposit_to_trading` ничего не делает.
- Минимального депозита в API нет — `venue.deposit_min_absent_ok` (решение 107).
- В живом `orderTypes` нет `IMMEDIATE_OR_CANCEL` (запись 2026-11-02) — проверяется пробным ордером, иначе `LIMIT`.
- Функции контракта только для DEX отвечают `NotApplicable`.
