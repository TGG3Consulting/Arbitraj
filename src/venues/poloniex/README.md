# venues/poloniex — адаптер Poloniex (спот, Spot API)

Реализует контракт `gateway/contract.rs` (решение 169). Всё — только по официальной документации Poloniex; анализ до
кода — `docs/notes/poloniex-analysis-2026-11-05.md`; чек-лист уроков (30 пунктов) и как каждый закрыт —
`docs/notes/poloniex-integration-2026-11-05.md`.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://api-docs.poloniex.com/spot/api/ | Адрес REST, подпись (key, signTimestamp, signature), лимиты запросов по VIP |
| https://api-docs.poloniex.com/spot/api/public/reference-data | Symbol Information (/markets), Currency Information V2 (/v2/currencies), /timestamp |
| https://api-docs.poloniex.com/spot/api/private/account | /accounts, /accounts/balances, /feeinfo |
| https://api-docs.poloniex.com/spot/api/private/wallet | /wallets/addresses, /wallets/activity, /v2/wallets/withdraw |
| https://api-docs.poloniex.com/spot/api/private/order | Create Order, Order Details, Cancel Order (id и cid:) |
| https://api-docs.poloniex.com/spot/api/private/trade | Trades by Order Id (feeAmount, feeCurrency) |
| https://api-docs.poloniex.com/spot/api/private/margin | Account Margin, Borrow Status (панель «Балансы») |
| https://api-docs.poloniex.com/spot/websocket/ | Адреса WS, подписка, ping каждые 30 с, лимиты |
| https://api-docs.poloniex.com/spot/websocket/market-data | Канал book (снимки 5/10/20), book_lv2 |
| https://api-docs.poloniex.com/spot/websocket/authentication | Вход auth в приватный WS |
| https://api-docs.poloniex.com/spot/websocket/balance | Канал balances |
| https://api-docs.poloniex.com/spot/error-code | Коды ошибок (кодов ключа, подписи, прав и лимита нет) |
| https://github.com/poloniex/polo-sdk-python | Официальный SDK: `polosdk/spot/rest/request.py` (подпись, формат ошибки code/message), `ws/client_authenticated.py` |

## Счета Poloniex

- **SPOT** — единственный счёт Spot API («SPOT is the default and only supported one»): торговля, депозит и вывод —
  с него, переводов между счетами нет (`deposit_to_trading` ничего не делает).
- Маржа идёт на том же спот-счёте (займы) — в панели «Балансы» только оценка и займы.
- Фьючерсы — отдельный API v3 с отдельными ключами: боту не нужны, в панели — «не читается».

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки, секреты (ключ, секрет), HTTP, журнал, сырые сообщения |
| `sign.rs` | Подпись: «METHOD\nпуть\nпараметры» (GET — по имени с signTimestamp, тело — requestBody=…&signTimestamp=…), HmacSHA256 → Base64; вход в WS «GET\n/ws\nsignTimestamp=…» |
| `rest.rs` | REST: строка запроса та же, что в подписи (кодирование SDK); резерв только при сбое сети |
| `reply.rs` | 2xx — тело как есть; иначе code и message (как SDK); 429 — пауза на Retry-After или venue.rate_limit_pause_ms |
| `errors.rs` | Коды Poloniex и HTTP-статусы → единые коды |
| `num.rs` | Числа (строки или числа JSON) и флаги без потери точности |
| `symbols.rs`, `symbols_parse.rs` | Справочник пар /markets: state, шаги из priceScale / quantityScale, мин./макс. количества и суммы, highestBid / lowestAsk |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs`, `market.rs` | Снимок пар и SpotPairs, фоновое обновление справочника, подписка (решение 131), поток аккаунта |
| `fees.rs` | Ставки /feeinfo одним запросом (special по паре, иначе общая); скидка TRX — расчёт по обычной ставке |
| `wallet.rs`, `networks_parse.rs` | Монеты и сети /v2/currencies (весь — вне рабочих потоков, за Arc; одна — /v2/currencies/{c}), адрес контракта, комиссия и минимум вывода |
| `deposit_addr.rs` | Адрес депозита: имя монеты в сети из /v2/currencies → /wallets/addresses?currency= (кэш; memo — не поддерживается) |
| `activity.rs`, `deposits.rs`, `transfers.rs` | Окно /wallets/activity (venue.history_window_s); статус депозита по txid; вывод v2 со своим withdrawOrderId, статус и поиск |
| `account.rs` | Балансы спота, «Проверить подключение» (подпись, accountState, ставки и скидка TRX; права — не проверяемы) |
| `all_balances.rs` | Панель «Балансы»: Спот, Маржа (оценка и займы), Фьючерсы (не читается — отдельные ключи) |
| `order_rules.rs` | Правила ордера — одни у ордера и у `check_order` (решение 134); свой id 17 цифр; пробного ордера нет |
| `trade.rs`, `order_state.rs`, `fills.rs` | Ордер LIMIT + IOC/FOK, итог по Order Details, отмена остатка, поиск по cid:, комиссия из сделок ордера |
| `depth.rs`, `ws.rs`, `ws_conn.rs`, `book.rs` | Канал book снимками (5/10/20), отсев по id (решение 109), ping, котировка; решение 202 — учёт свежести для сторожа ядра (gateway/feed_reg.rs), переподписка одной пары (unsubscribe + subscribe канала book) и переподключение одного соединения по команде сторожа; решение 213 — срок подтверждения тихой пары (`book_confirmed`) |
| `book_print.rs` | Решение 213 по содержимому: `GET /markets/{symbol}/orderBook` (limit = `venue.depth_levels`) — первые N уровней ответа и стакана в памяти для подтверждения тихой пары ядром; Решение 238: тот же запрос — снимок стакана для ордера (`snapshot`, контракт `quote_rest`), все уровни ответа |
| `user_stream.rs`, `account_update.rs` | Поток аккаунта: вход auth, канал balances, изменение = разница остатков (опора — REST при входе) |

## Чего нет в документации (не угадано — проверить на живых ответах, вопросы Тиграну)

- **Имя сети BSC** (`blockchain` в /v2/currencies) — только из живого ответа → `venue.bsc_network`.
- **Адрес депозита BSC**: у /wallets/addresses нет параметра сети; берётся имя монеты в сети (`coin` сети в
  /v2/currencies) — проверить, что адрес возвращается под этим именем.
- **Минимум депозита** — нет в API → `venue.deposit_min_absent_ok` (урок 21).
- **Точность суммы вывода** — `decimals` назван «Decimals of the currency»: смысл не ясен → не используется.
- **Входит ли комиссия в amount вывода** — не описано (в истории amount «including the fee»).
- Решение 232: котировка (`book.rs`) — комиссия покупки в монете, продажи в USDT (примеры Spot → Trades: BUY — feeCurrency LINK,
  SELL — USDT); скидка TRX — без неё. Факт из сделок ордера в нескольких монетах — список частей (`fills.rs`).
- **Статусы**: депозит COMPLETED (док.) / COMPLETE (SDK); вывод PROCESSING (док.) / PENDING (SDK) — принимаются оба.
  SDK пишет, что в status вывода «optionally the transaction ID» — формат не описан: неизвестное значение — ошибка с сырым
  ответом.
- **clientOrderId** — 64 знака (Create Order) против «17 digits» (код 21315): свой id — 17 цифр, проходит оба.
- **IOC** в справочнике пары не перечисляется (урок 30) — подтвердить первым живым ордером.
- **Формат ошибки, ответ при превышении лимита, коды ключа/подписи/прав/IP** — не описаны: 429 → пауза
  `venue.rate_limit_pause_ms`, остальное — по HTTP-статусу, текст — message ответа.
- **Права ключа** — эндпоинта нет: «Проверить подключение» проверяет подпись и чтение, права торговли и вывода — нет
  (урок 29). Белый список IP и адресов вывода — только кабинет.
- **Предел символов в подписке и на соединение** — нет: подписка по одному символу, `venue.ws_symbols_per_conn` — Тигран.
- **Уведомление balances** без поля изменения — изменение = разница остатков (available + hold); хеша нет →
  `transfer.credit_by_event` у Poloniex до живой записи не включать.
- **Удержание вывода (решение 150)** — статуса «предварительно зачислено» нет → `withdraw_hold` пусто.
- **Скидка TRX** — размер не описан: расчёт по обычной ставке, факт — из сделок (feeCurrency).

## Что важно знать

- WebSocket: публичный `venue.ws_url`, приватный `venue.ws_private_url`; `{"event":"ping"}` раз в `venue.ws_ping_ms`
  (< 30 000), тишина `venue.ws_silence_ms`; ≤ 500 сообщений в секунду на соединение.
- Лимиты REST — по UID (приватные) и IP (публичные), значения по VIP; `/wallets/activity` и вывод — 10 (единица времени
  в таблице приватных не подписана) — не опрашивать часто.
- Сторож пар (решения 202/213, 08.11): `feed_health` — scheduled = false, per_pair = true, **confirm = true — по
  содержимому**: у кадра book есть id («id of the record (SeqId)»), а в ответе REST стакана номера нет; сравниваются
  первые `venue.depth_levels` уровней ответа и стакана в памяти (`book_print.rs`). Лимит Order Book в таблице «Rate
  Limits» пустой (рыночные — по IP; /markets/{symbol} — 200/с).
- Функции контракта только для DEX отвечают `NotApplicable`.
