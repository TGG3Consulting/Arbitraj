# venues/xt — адаптер XT.com (спот, API v4, решение 167)

Реализует контракт `gateway/contract.rs`. Всё — только по официальной документации XT; разбор перед подключением —
`docs/notes/xt-analysis-2026-11-05.md`. Живых ответов на момент написания нет (из среды разработки `sapi.xt.com` закрыт
прокси): всё, что помечено «проверить на живом», сверяется по записям (tests/README.md, раздел «XT»).

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://doc.xt.com/ | Документация API v4 (одна страница, разделы по якорям `#<раздел><файл>`) |
| https://github.com/xt-com/xt4-api (ветка main, коммит `8edf738` от 22.01.2025) | Исходник doc.xt.com: `_documentation/` (подпись, лимиты, коды, publicModule), `_market/` (symbol, depth), `_order/`, `_trade/`, `_balance/`, `_deposit_withdrawal/`, `_transfer/`, `_websocket_public/`, `_websocket_private/`, `_user_center_apikey/`, `_futures_user/`, `_futures_documentation/` |
| https://github.com/kelvinxue/pyxt (коммит `db336a1`) | SDK, на который ссылается документация: эндпоинты совпадают; заголовки подписи `xt-validate-*` — расхождение с документацией |

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки, секреты `xt.api_key` / `xt.api_secret`, HTTP, журнал; тикер ядра заглавными ↔ имя монеты XT строчными |
| `sign.rs` | Подпись: X (заголовки `validate-*` по алфавиту) + Y (`#METHOD#path#query#body`), HmacSHA256 hex; фьючерсы — X без алгоритма и окна, Y без метода |
| `rest.rs`, `reply.rs`, `errors.rs` | REST: ответ `{rc, mc, ma, result}`, recvWindow ≤ 60000, 429 и `GATEWAY_*` — пауза `venue.rate_limit_pause_ms` (отдельно спот и прочие адреса); ответ с секретом ключа (user-center) не пишется в сырой журнал; коды mc → единые («XT, код <mc>: …» — решение 164) |
| `resolve.rs` | Решение 231: исход операции «неизвестен» по «чем найти» из ошибки — ордер по своему ID / ID ордера (те же запрос, отмена и доведение, что у постановки); вывод — по своему ID в истории выводов |
| `num.rs` | Числа без потери точности (строки и числа JSON) |
| `symbols.rs`, `symbols_parse.rs` | `/v4/public/symbol` (вне рабочих потоков, с `version` — без изменений список не приходит): state, tradingEnabled, openapiEnabled, PRICE / QUANTITY (шаг от min, null — без предела), QUOTE_QTY, точности, takerFeeRate, orderTypes, timeInForces |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs`, `market.rs` | Снимок пар (SpotPairs), фоновое обновление, подписка (как Binance) |
| `order_rules.rs` | ОДНА функция правил ордера (решение 134): у ордера и у `check_order`; LIMIT + IOC/FOK, округление от min, пределы, QUOTE_QTY; пробного ордера у XT нет |
| `trade.rs`, `order_state.rs` | `POST /v4/order` (тело JSON, свой clientOrderId), итог `GET /v4/order/{id}`, отмена `DELETE`; поиск по clientOrderId при неизвестном итоге; комиссия — `deductServices` или `fee`/`feeCurrency` |
| `book_version.rs` | Решения 202/213: версия стакана пары по REST `GET /v4/public/depth` для сторожа ядра — при `depth_update` номер (`lastUpdateId` ↔ номер локального стакана, «Orderbook manage»), при `depth` — первые `venue.depth_levels` уровней (по содержимому); Решение 238: тот же запрос — снимок стакана для ордера (`snapshot`, контракт `quote_rest`), все уровни ответа |
| `book.rs`, `depth.rs`, `ws.rs`, `ws_conn.rs`, `ws_book.rs`, `local_book.rs`, `snapshots.rs` | Сторож пар (решение 202): учёт свежести на каждом кадре, переподписка пары `unsubscribe` + `subscribe`, переподключение соединения; подтверждённая тихая пара свежа до срока (`book.rs`). Стаканы: `depth@{symbol},{levels}` (снимки 1000 мс; тот же updateId — без отдачи ядру) или `depth_update@{symbol}` (100 мс) + снимок REST `/v4/public/depth` из общей очереди (≤ 10/с); текст `ping` раз в `venue.ws_ping_ms`; котировка по ставке тейкера пары |
| `wallet.rs`, `networks_parse.rs` | `/v4/public/wallet/support/currency` + `/v4/public/currencies` (кэш): сеть BSC (`chain`), `contract`, ввод/вывод по сети и по монете, комиссия и минимум вывода; ненулевой `depositFeeRate` — ввод по сети закрыт; минимум депозита — `venue.deposit_min_absent_ok` |
| `deposit_addr.rs` | Адрес депозита, кэш (монета, сеть); memo — отказ; DEPOSIT_006/007 — «адрес создаётся» |
| `transfers.rs` | Вывод `POST /v4/withdraw` (сумма включает комиссию, свой clientOrderId), статус `GET /v4/withdraw?recordId=`, поиск по clientOrderId; 1 запрос/с |
| `deposits.rs` | Депозит по хешу: история монеты в сети BSC за `venue.history_window_s` (фильтра по хешу у XT нет) |
| `account.rs`, `all_balances.rs`, `key_check.rs` | Баланс спота (все и одна монета); панель «Балансы» (Спот, Фьючерсы USDT-M и Coin-M, Маржа — примечание); «Проверить подключение» (GET /v4/balances, запись ключа в user-center) |

## Что важно знать

- Подпись — заголовки `validate-algorithms`, `validate-appkey`, `validate-recvwindow`, `validate-timestamp`,
  `validate-signature`. Passphrase нет. recvWindow в документации противоречив (5000 «не больше», пример 60000).
- Лимиты: 429 — «IP или apiKey будет заблокирован», Retry-After нет. Рынок 10/с на IP, `/v4/balances` 10/с на ключ,
  `POST /v4/order` 20/с, `/v4/withdraw` (вывод и статус) 1/с. Лимитов WebSocket (тем на соединение, сообщений в
  секунду, соединений с IP) в документации нет — подбор живым замером (решение 109).
- WebSocket: документация требует заголовок `Sec-Websocket-Extensions: permessage-deflate`; tokio-tungstenite 0.30
  сжатие не умеет — подключение без заголовка. Если XT не отдаёт кадры без сжатия — нужна другая библиотека
  WebSocket (решение Тиграна).
- Ставки комиссии по ключу в API нет — расчёт по `takerFeeRate` пары; скидка оплатой в XT (`deductServices`) — только
- Решение 232: покупка — комиссия в монете (факт бота 08.11: WIN, 0.002 от количества); `deductServices` в нескольких монетах —
  список частей (`order_state.rs fee`), не ошибка.
  по факту ордера (урок 23).
- Поток аккаунта не подключён (урок 16): тема `balance` присылает итоговый баланс (`b`, `f`) без причины изменения —
  депозит нельзя отличить от сделки (как OKX `account`); зачисление — опросом `deposit_status`.
- Удержание вывода T+1 (`WITHDRAW_009`) заранее через API не узнать — `withdraw_hold` пустой (урок 22).
- Счёт зачисления депозита не описан; внутренний перевод `POST /v4/balance/transfer` есть, но не подключён до живого
  депозита (если депозит придёт не на SPOT — `deposit_to_trading`, вопрос Тиграну).
- Права ключа: QUERY + TRADE (спот и маржа одним правом) + WITHDRAW; без FUTURE и LEVER_LOAN. Адрес кошелька DEX,
  вероятно, нужно заранее добавить в адресную книгу (WITHDRAW_004).

## Проверить на живых ответах (вопросы — в отчёте решения 167)

1. Заголовки `validate-*` (документация) или `xt-validate-*` (SDK pyxt); hex строчными или заглавными (AUTH_103).
2. Принимает ли `wss://stream.xt.com/public` клиента без permessage-deflate.
3. Название сети BSC в `chain` (`venue.bsc_network`), поле `contract` у CAKE и USDT (регистр), у BNB.
4. `depositFeeRate` у сети BSC: 0 или число (смысл — вопрос Тиграну); регистр `currency` в справочнике и балансах.
5. Типы чисел (строка/число) в `/v4/public/symbol`, `null` в фильтрах, `version` без изменений (поле `symbols`?).
6. Итог IOC с остатком — CANCELED или EXPIRED; монета комиссии покупки; `deductServices` при оплате в XT.
7. `fi`/`i` в `depth_update` и `lastUpdateId` снимка — согласованы ли; одинаковый `i` у снимков `depth`.
8. Ответ `/v4/withdraw?recordId=` (объект или null), `clientOrderId` строкой; ответ «фьючерсный счёт не открыт».
9. Доступен ли `/v4/user/account/api-key` обычному ключу; значения `roleScopes`.
