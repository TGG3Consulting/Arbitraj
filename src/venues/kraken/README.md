# venues/kraken — адаптер Kraken (спот)

Реализует контракт `gateway/contract.rs` (решение 172). Только спот: ордер `limit` без `leverage`, комиссия в котируемой
монете (`oflags=fciq`). Всё — только по официальной документации Kraken; разбор перед подключением —
`docs/notes/kraken-analysis-2026-11-05.md`, как закрыт каждый из 30 уроков — `docs/notes/kraken-integration-2026-11-05.md`.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://docs.kraken.com/openapi/spot-rest.yaml | OpenAPI спот REST и Funding (Beta): пути, поля, перечни |
| https://docs.kraken.com/exchange/guides/rest/authentication.md | Подпись спот REST (API-Key, API-Sign, nonce), проверочный пример AddOrder |
| https://docs.kraken.com/exchange/guides/rest/funding.md | Funding (Beta): подпись (api-key, api-sign, api-nonce; путь со строкой запроса), адреса, address_id |
| https://docs.kraken.com/api-reference/funding-beta/list-funding-methods.md | Методы ввода/вывода: method_id, network_name, contract_address, комиссии, минимумы |
| https://docs.kraken.com/api-reference/funding-beta/create-funding-withdrawal.md | Вывод: scope, address_id, amount, fee, expected_address, approval_request_id |
| https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/book.md | Канал book: подписка, снимок, изменения, checksum |
| https://docs.kraken.com/exchange/guides/websockets/book-checksum-v2.md | Чек-сумма стакана CRC32 (10 уровней, формат без точки и ведущих нулей) |
| https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/balances.md | Канал balances (ws-auth, токен) |
| https://docs.kraken.com/exchange/guides/rest/ratelimits.md, …/general/ratelimits.md | Счётчик REST на ключ, счётчик торгового движка на пару |
| https://support.kraken.com/articles/360000674066 | Статусы депозита (Credited — торговать можно, выводить нельзя) |

Из среды разработки `docs.kraken.com` и `api.kraken.com` закрыты прокси — документация прочитана пересказчиком, живых
ответов нет. **Каждое поле сверить на живых записях** (команды — `tests/README.md`, раздел Kraken).

## Устройство

- **Две части API.** Спот REST `https://api.kraken.com/0/…` (пары, ордера, балансы, ставки, токен WebSocket,
  `DepositStatus`) и Funding (Beta) `https://api.kraken.com/funding/…` (методы с адресом контракта, адресная книга,
  выводы, депозиты, адреса депозита). `venue.rest_url` — `https://api.kraken.com` без `/0`: пути адаптер добавляет сам.
- **Один поток запросов на ключ.** nonce у Kraken только растёт; запросы одним ключом не по порядку — «EAPI:Invalid
  nonce», повторы — временный бан. Все подписанные запросы идут через полосу `ctx.lane`: nonce выдаётся и запрос
  отправляется под одним замком, следующий — после ответа. Ключ бота не должен использоваться другими программами.
- **Счёт один (спот).** Депозит зачисляется на спот, вывод — со спота: переводов между счетами нет.
- **Вывод только на адрес из адресной книги** (`address_id`). Адреса (кошельки DEX, депозиты других бирж) Тигран
  добавляет в кабинете Kraken; право ключа «Add withdrawal addresses» не даётся. Комиссия вывода — котировкой на сумму
  (`/funding/v1/fees`, токен 5 минут), внутри суммы (`fee_included = true`); больше комиссии справочника — отказ.
- **Стакан** — канал `book` WebSocket v2: снимок + изменения, номеров нет — чек-сумма CRC32 верхних 10 уровней после
  каждого кадра (формат по `pair_decimals` / `lot_decimals` пары). Не совпала — пересподписка пары; второй раз раньше
  `venue.ws_reconnect_pause_ms` — переподключение. Подключения — не чаще одного на 4 с на все соединения адаптера
  (Cloudflare: ~150 попыток за 10 минут на IP).
- **Приватный поток** — канал `balances` на `venue.ws_private_url` с новым токеном на каждое подключение: депозиты и
  выводы спота → `MarketEvent::Balance` (без хеша — будит проверку статуса).

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки, секреты (ключ, приватный ключ Base64), HTTP, журнал, сырые сообщения, полоса nonce |
| `sign.rs` | API-Sign = Base64(HMAC-SHA512(путь + SHA256(nonce + данные), Base64-декод секрета)); следующий nonce |
| `rest.rs` | Публичные GET /0/public, подписанные POST /0/private (форма), подписанные /funding (строка запроса + JSON) |
| `reply.rs` | Разбор {error, result} и ответов Funding; паузы «private», «trade», путь — после «Rate limit exceeded» / Throttled |
| `errors.rs` | Строки ошибок Kraken → единые коды; текст «Kraken, ошибка …» (notify/reason.rs, решение 164) |
| `num.rs` | Числа и поля без потери точности; суммы Funding {asset, amount} / {asset_amount} |
| `symbols.rs`, `symbols_parse.rs` | Справочник `AssetPairs`: wsname, tick_size, lot_decimals, pair_decimals, ordermin, costmin, status |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs`, `market.rs` | Снимок пар и SpotPairs, фоновое обновление, подписка по соединениям |
| `fees.rs` | Ставки `TradeVolume` пачками пар (fee в процентах) фоном |
| `methods.rs`, `networks_parse.rs`, `wallet.rs` | Методы Funding (все страницы / одна монета) + `Assets`: сети монет, адрес контракта, комиссия, минимумы, method_id |
| `deposit_addr.rs` | Адрес депозита: выданные (v2), новый — только при `venue.deposit_address_claim`; кэш с expire_time; memo — отказ |
| `deposits.rs` | Статус депозита по хешу (список депозитов), удержания вывода — `DepositStatus` EarlyConfirmed (решение 150) |
| `withdraw_addr.rs`, `transfers.rs` | address_id по адресу, котировка комиссии, вывод, статус и поиск вывода при неизвестном итоге |
| `account.rs` | Балансы `BalanceEx` (свободно = balance − hold_trade), «Проверить подключение» (`GetApiKeyInfo`, Funding, сеть BSC) |
| `all_balances.rs` | Панель «Балансы» (решение 144): спот, прочие кошельки (.B .F .T .S .M), фьючерсы — только сведения |
| `order_rules.rs` | Правила ордера — одни у ордера и `check_order` (решение 134); пробный ордер `validate=true` |
| `trade.rs`, `order_state.rs` | Ордер limit IOC/FOK со своим cl_ord_id, итог `QueryOrders` (vol_exec, price, fee), отмена, поиск по cl_ord_id |
| `depth.rs` | Разбор кадров WS v2: pong, ответы, heartbeat/status, book, balances |
| `local_book.rs`, `checksum.rs` | Локальный стакан (снимок + изменения, обрезка до глубины) и CRC32 |
| `ws.rs`, `ws_conn.rs`, `ws_req.rs`, `book.rs` | Соединения стаканов: ограничитель подключений, подписка пачками, ping, пересподписка, котировка; решение 202 — учёт свежести для сторожа ядра (gateway/feed_reg.rs), переподписка одной пары (unsubscribe + subscribe, `ws_req.rs resub`) и переподключение одного соединения по команде сторожа; решение 213 — срок подтверждения тихой пары (`book_confirmed`) |
| `book_print.rs` | Решение 213 по содержимому: `GET /0/public/Depth` (count = `venue.depth_levels`, не больше 500) — первые N уровней ответа и стакана в памяти для подтверждения тихой пары ядром |
| `user_stream.rs`, `user_stream_conn.rs`, `balance_event.rs` | Приватный поток balances → MarketEvent::Balance |

## Что важно знать

- Счётчик REST ключа маленький (Starter 15, убывание 0,33 в с): ставки — пачками (`venue.fee_pairs_per_request`),
  панель «Балансы» (запрос раз в 3 с) не держать открытой при торговле на уровне Starter.
- Процентная комиссия вывода, платный ввод или плата за адрес, два метода одной монеты на одной сети — не угадываются:
  сеть закрывается с записью в журнале (`venue.networks`).
- Скидки своей монетой у Kraken нет — урок 23 (решение 158) неприменим.
- Запись окна (решение 125) для Kraken не сделана — `window_tap` отвечает NotApplicable (по умолчанию контракта).
- Сторож пар (решения 202/213, 08.11): `feed_health` — scheduled = false, per_pair = true, **confirm = true — по
  содержимому**: номера нет ни у кадра book, ни у ответа REST; сравниваются первые `venue.depth_levels` уровней ответа
  GET /0/public/Depth и стакана в памяти (`book_print.rs`). У REST count — не больше 500: при `venue.depth_levels` = 1000
  подтверждения нет (ошибка с причиной). Лимит публичных запросов в документации не описан.
- Функции контракта только для DEX отвечают `NotApplicable`.
