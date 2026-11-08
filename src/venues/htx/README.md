# venues/htx — адаптер HTX (бывший Huobi; спот, API v1/v2)

Реализует контракт `gateway/contract.rs` (решение 171). Всё — только по официальной документации HTX и официальному
SDK; чек-лист уроков (30 пунктов) и как каждый закрыт — `docs/notes/htx-integration-2026-11-05.md`; анализ до кода —
`docs/notes/htx-analysis-2026-11-05.md`.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://huobiapi.github.io/docs/spot/v1/en/ | Spot API целиком: адреса, подпись V2, форматы ответов, лимиты, Reference Data, Market Data, Account, Wallet, Trading, WebSocket рынка и аккаунта |
| https://github.com/HuobiRDCenter/huobi_Golang | Официальный SDK: `internal/requestbuilder` (подпись — векторы тестов), `pkg/model/*` (поля ответов), `pkg/client/*` (пути) |
| https://www.htx.com/en-us/opend/newApiPages/ | Новый портал документации — из среды разработки **не открылся**, расхождения со старой страницей не проверены |

## Счета HTX

- **spot** — торговля и баланс бота (`/v1/account/accounts`, тип spot; id спот-счёта нужен ордеру и балансу).
- Отдельного фонда у спот-API нет: депозит зачисляется на спот (подтвердить первым депозитом), переводов между счетами
  в адаптере нет (`deposit_to_trading` — ничего не делает).
- Вывод — только на адреса из списка адресов вывода (добавляются **только в веб-интерфейсе HTX**); бот проверяет адрес
  по списку до вывода. Поле `chain` уходит всегда (без него у USDT — ERC20).

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки, секреты (ключ, секрет), HTTP, журнал, сырые сообщения; тикер ↔ имя монеты HTX |
| `delist.rs` | Решение 226: делистинги — пары V1 GET /v1/settings/common/symbols с venue.quote_asset, state не offline и `dl` = true («delist»), время — `tca` («trade-close-at»); dl = false не учитывается (что значит tca без dl — вопрос Тиграну) |
| `sign.rs` | Подпись V2: кодирование и сортировка параметров, HMAC-SHA256 → Base64, строка запроса REST, вход WebSocket v2 |
| `rest.rs` | REST: GET — параметры в подписи; POST — тело JSON, в строке только подпись; резерв адреса только при сбое сети |
| `reply.rs` | Разбор v1 {status, err-code, data} и v2 {code 200, message, data}; HTTP 429 — пауза всех запросов |
| `errors.rs` | Коды HTX → единые коды («HTX, код …: …» — для человеческих причин, решение 164) |
| `num.rs` | Числа (строкой или числом JSON) без потери точности |
| `accounts.rs` | Список счетов, id спот-счёта («счёт не открыт» — урок 27) |
| `symbols.rs`, `symbols_parse.rs` | Справочник пар market-symbols: state, шаги 10^−pp / 10^−ap, минимумы и максимумы, minov |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs`, `market.rs` | Снимок пар и SpotPairs, фоновое обновление справочника, подписка (решение 131), поток аккаунта |
| `fees.rs` | Ставки ключа transact-fee-rate пачками; худшая ставка тейкера (скидка HT — не в расчёте) |
| `wallet.rs`, `networks_parse.rs` | Монеты и сети /v2/reference/currencies (+ адрес контракта из /v1/settings/common/chains по venue.contract_field) |
| `deposit_addr.rs` | Адрес депозита (кэш; addressTag — не поддерживается) |
| `records.rs`, `deposits.rs` | История депозитов и выводов; статус депозита по хешу; удержания вывода (confirmed ≠ safe) |
| `transfers.rs` | Вывод со своим client-order-id, проверка списка адресов вывода, статус по client-order-id |
| `order_rules.rs` | Правила ордера — одни у ордера и у `check_order` (решение 134); пробного ордера нет |
| `trade.rs`, `order_state.rs`, `order_fee.rs` | Ордер buy-ioc / sell-ioc (или limit-fok), итог по детали ордера, отмена остатка, поиск по client-order-id, комиссия из matchresults |
| `depth.rs`, `ws.rs`, `ws_conn.rs`, `book.rs` | Стаканы mbp.refresh (GZIP, ping/pong), отсев по seqNum (решение 109), котировка |
| `user_stream.rs`, `user_stream_conn.rs`, `account_update.rs` | Поток аккаунта WebSocket v2: вход подписью, accounts.update#2 → изменение баланса |
| `account.rs` | Балансы спота, «Проверить подключение» (своя запись ключа /v2/user/api-key, спот-счёт, поле адреса контракта) |
| `all_balances.rs` | Панель «Балансы»: все счета пользователя по списку |

## Чего нет и почему (вопросы Тиграну)

- **Адрес контракта BEP20** — в документации нет. В Get Chains Information — `ac` «address of chain», в SDK — `ca` без
  описания. Без `venue.contract_field` (ac / ca — по живому ответу) адресов нет, монеты HTX **не сопоставляются**.
- **Имя сети BSC** — в документации нет; `chain` у HTX своё у каждой монеты, поэтому сеть ядра — `baseChainProtocol`
  (`venue.bsc_network`, по живому ответу).
- **Комиссия вывода circulated / ratio** — как считает HTX, не описано: комиссия неизвестна, вывода нет (только fixed).
- Решение 232: комиссия покупки — в монете, продажи — в котируемой (fee-currency matchresults); факт со скидкой HT / баллами и в
  монете сделки одновременно — список частей (`order_fee.rs`), не ошибка. Баллы HB (fee-deduct-currency «hbpoint») —
  часть HBPOINT со стоимостью в USDT 1:1 (`Fee.usd`; справка HTX «Point Card Usage Instructions»: «1 pts = 1 USDT fee»;
  API цены баллов нет) — в затраты сделки (решение 232, дополнение).
- **Вывод confirm-error** — итог неясен: ждём (Pending), запись в журнал.
- **Ответ на превышение лимита REST** — не описан: пауза только по HTTP 429; заголовки X-HB-RateLimit-* транспорт шлюза
  не отдаёт.
- **Скидка HT / баллами HB** — состояние переключателя в документации не читается: расчёт по худшей ставке, факт — из
  matchresults (монета скидки — HT / HBPOINT).
- **Предел тем на соединение WS, символов в transact-fee-rate** — не указаны: настройки задаёт Тигран по замеру.
- **filled-amount / field-amount** — документация и SDK пишут по-разному: принимаются оба.
- **Окна (решение 125, window_tap)** — не подключены (контракт отвечает NotApplicable).
- **Фьючерсы** в панели «Балансы» — отдельный API деривативов, не читаются.

## Что важно знать

- WebSocket рынка: `venue.ws_url`, кадры GZIP; ping шлёт сервер (~5 с) — бот отвечает pong; тишина `venue.ws_silence_ms`.
- Поток аккаунта: `venue.ws_private_url` (wss://api.huobi.pro/ws/v2), без сжатия, ping сервера ~20 с —
  `venue.ws_silence_ms` больше 20000.
- Лимиты REST: «N times/2s» на эндпоинт по UID (все ключи UID вместе); публичные — по IP.
- Функции контракта только для DEX отвечают `NotApplicable`.
