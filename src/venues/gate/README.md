# venues/gate — адаптер Gate (Gate.io, спот, API v4)

Реализует контракт `gateway/contract.rs` (решение 135). Всё — только по официальной документации Gate; разбор и
чек-лист уроков BingX/MEXC — `docs/notes/gate-integration-2026-11-04.md`.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://www.gate.com/docs/developers/apiv4/en/ | Основная документация API v4: Access URL, Authentication (подпись), Frequency limit rule, Error label list, API key permissions, эндпоинты |
| https://www.gate.com/docs/developers/apiv4/ws/en/ | Spot WebSocket v4: формат запросов, spot.ping/pong, spot.order_book, spot.order_book_update, spot.trades, spot.balances, подпись приватных каналов |
| https://github.com/gate/gateapi-python (v4.106.179) | Официальный SDK, сгенерирован из спецификации: docs/*.md — поля моделей и параметры методов (CurrencyPair, Currency, CurrencyChain, Order, WithdrawalRecord, DepositRecord, LedgerRecord, WithdrawStatus, DepositAddress, AccountDetail, AccountKeyInfo); gate_api/api_client.py — gen_sign |
| https://github.com/gate/gatews | Официальный SDK WebSocket: имена каналов (go/constant.go), подпись (python/gate_ws/client.py), модели уведомлений (go/response_spot.go) |
| tests/data/cex_samples_2026_11_04/gate_* | Живая запись публичных ответов 04.11.2026 (REST и WebSocket) |

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки площадки, секреты, HTTP, журнал, сырые сообщения; символ пары `CAKE_USDT` |
| `rest.rs` | REST: подпись HMAC-SHA512 (KEY, Timestamp, SIGN; путь с /api/v4, строка запроса без кодирования, SHA512 тела), JSON-тело; резерв — только при сбое сети |
| `reply.rs` | Разбор ответа: 2xx — JSON; иначе {label, message}; 429 / TOO_MANY_REQUESTS — пауза (Retry-After или venue.rate_limit_pause_ms) |
| `errors.rs` | Метки Gate → единые коды (INVALID_KEY/SIGNATURE → ключ; REQUEST_EXPIRED → «часы»; IP_FORBIDDEN; READ_ONLY/FORBIDDEN → права; BALANCE_NOT_ENOUGH; ADDRESS_NOT_USED → адрес; 5xx и TOO_BUSY у ордера/вывода → итог неизвестен) |
| `num.rs` | Числа из ответов без потери точности |
| `symbols.rs`, `symbols_parse.rs` | Справочник пар (/spot/currency_pairs + белый список пар ключа /account/detail): trade_status, шаги 10^−precision, минимумы и максимумы, buy_start/sell_start |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs` | Снимок пар (SpotPairs), фоновое обновление справочника, подписка (как MEXC; комиссии не перезапускаются при том же списке — решение 131) |
| `fees.rs` | Ставки комиссии по паре (`GET /wallet/fee?currency_pair=`) фоном; вариант списания venue.fee_deduction (coin / gt) и его сверка со скидкой GT счёта |
| `wallet.rs`, `wallet_cost.rs`, `networks_parse.rs` | Монеты и сети (/spot/currencies + /wallet/withdraw_status) за `Arc` с кэшем venue.networks_fresh_ms; одна монета — /spot/currencies/{c}, withdraw_status?currency=, /wallet/currency_chains (точность вывода decimal); комиссия вывода — withdraw_fix_on_chains |
| `delist.rs` | Решение 175: делистинги — `delisting_time` пар GET /spot/currency_pairs («Expected delisting time as a Unix timestamp in seconds»), только пары с venue.quote_asset; времени закрытия вывода в модели нет |
| `deposit_addr.rs` | Адрес депозита (/wallet/deposit_address, multichain_addresses по chain): memo — отказ, obtain_failed — отказ; минимум депозита и подтверждения — в справочник |
| `deposit_info.rs` | Решение 143: фоновый круг адресов депозита по монетам подписки — минимум депозита (min_deposit_amount) и подтверждения в справочник сетей; venue.deposit_info_requests_per_sec, venue.deposit_info_refresh_s |
| `all_balances.rs`, `all_balances_parse.rs` | Решение 144: панель «Балансы» — все счета Gate (спот, маржа, фьючерсы по venue.futures_settles, поставочные по venue.delivery_settles, опционы, Earn, итог /wallet/total_balance), только чтение |
| `transfers.rs`, `deposits.rs` | Вывод POST /withdrawals (withdraw_order_id на попытку), статус и поиск по withdraw_order_id; статус депозита по хешу (/wallet/deposits) |
| `deposit_hold.rs` | Решение 166: удержания вывода — депозиты DEP_CREDITED («withdrawal is not unlocked», GET /wallet/deposits). Метки «заблокировано для вывода» в списке меток Gate нет — WithdrawLocked не выдаётся |
| `account.rs` | Балансы /spot/accounts (все и одной монеты); «Проверить подключение»: key.mode, state, perms, IP |
| `order_rules.rs` | Правила ордера — одни у ордера и у check_order (решение 134): сторона через API, ioc/fok, шаги, пределы; свой text «t-…» |
| `trade.rs`, `order_state.rs` | Ордер limit + ioc или fok (venue.order_time_in_force); итог из полей ордера; опрос, отмена по сроку, поиск по text (ожидающие → отмена → законченные) |
| `withdraw_notice.rs` | Вывод в статусе FAIL — журнал и событие exec.withdraw_fail_pending, один раз на вывод (вывод не проваливается — ждём) |
| `depth.rs`, `market.rs`, `ws.rs`, `ws_conn.rs`, `book.rs` | WebSocket стаканов spot.order_book (снимки N уровней, отсев по lastUpdateId — решение 109), spot.ping, котировка, свежесть по соединению и кадру пары |
| `user_stream.rs`, `account_update.rs` | Поток балансов spot.balances с подписью (решение 76) |
| `tap.rs`, `feed.rs`, `tap_snap.rs` | Окна (решение 125): кадры стакана, spot.trades, spot.order_book_update + снимок REST по U/u |

## Что важно знать

- Депозит зачисляется на спот («DONE: Funds Credited to Spot Account») — `deposit_to_trading` ничего не делает.
  Зачислено = DONE или DEP_CREDITED (продаём сразу, решение Тиграна 04.11).
- Вывод FAIL («On-Chain Failure Pending Confirmation») — не провал: ждём следующего статуса, сразу журнал и событие
  exec.withdraw_fail_pending.
- Комиссия (решение Тиграна 04.11): venue.fee_deduction = coin — в монете сделки (taker_fee; покупка уменьшает
  количество), gt — в GT (gt_taker_fee; монета не уменьшается). Не совпадает со скидкой GT счёта — котировки нет.
  Что Gate делает при нехватке GT — в документации API нет (вопрос в docs/TASKS.md). Факт — из ордера.
- Пробного ордера в спот API v4 нет: `venue.order_test_first` = да для Gate — ошибка настройки.
- Ключ — только классический режим (key.mode = 1). Единый счёт не поддержан.
- Лимиты: постановка ордера — 10/с на UID и рынок, отмена — 200/с, прочие приватные — 200 за 10 с, публичные — 200 за
  10 с на IP. Retry-After в документации нет — пауза venue.rate_limit_pause_ms.
- Сеть BSC у Gate — «BSC» (живая запись 04.11).
- Функции контракта только для DEX отвечают `NotApplicable`.
