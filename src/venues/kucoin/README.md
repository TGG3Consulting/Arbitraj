# venues/kucoin — адаптер KuCoin (спот, Classic API)

Реализует контракт `gateway/contract.rs` (решение 137). Всё — только по официальной документации KuCoin; чек-лист
уроков BingX/MEXC и как каждый закрыт — `docs/notes/kucoin-integration-2026-11-04.md`.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://www.kucoin.com/docs-new/ (раздел Classic) | Документация: подпись, лимиты, эндпоинты, коды ошибок, WebSocket |
| https://www.kucoin.com/docs-new/authentication | Заголовки KC-API-*, строка подписи, шифрование passphrase |
| https://www.kucoin.com/docs-new/rate-limit-rule-classic | Ресурсные пулы, квоты за 30 с, 429000, пределы WebSocket |
| https://www.kucoin.com/docs-new/error-code/spot, …/error-code/http | Коды ошибок |
| https://www.kucoin.com/docs-new/websocket-api/base-info/introduction | welcome, ping/pong, subscribe/ack, токен 24 ч |
| https://github.com/Kucoin/kucoin-universal-sdk (v1.3.2, коммит c2f39bd от 2026-09-15) | Официальные OpenAPI-спецификации `spec/rest/api/*.json`, `spec/ws/*.json` (поля, пулы `x-api-rate-limit-pool`, веса, права `x-api-permission`, темы `x-topic`); подпись SDK и её тестовый вектор |
| https://www.kucoin.com/announcement/en-announcement-of-kucoin-api-upgrade-2026-06-03 | UTA и Classic: «Continue using the Classic API for Classic Account trading» |

Версия API: **Classic** (классический счёт). UTA API — отдельный слой для счёта в режиме Unified Trading Account
(переход по выбору: «Main accounts and sub-accounts can each be switched … independently»); адаптер его не использует —
счёт бота должен оставаться классическим (вопрос Тиграну — docs/TASKS.md).

## Счета KuCoin

- **main** (funding) — сюда приходит депозит (по умолчанию: «Deposit account type … the default is MAIN»), отсюда идёт
  вывод (код 115007 «withdrawals only come from the funds account»).
- **trade** — торговый счёт: на нём ордера и баланс бота (`GET /api/v1/accounts?type=trade`).
- Перевод main ↔ trade — Flex Transfer `POST /api/v3/accounts/universal-transfer` (type INTERNAL, право FlexTransfers);
  прежний `POST /api/v2/accounts/inner-transfer` в спецификации помечен «abandon». После зачисления — main → trade
  (`venue.deposit_to_spot`; не нужно, если адрес депозита сам зачисляет на TRADE), перед выводом — trade → main
  (`venue.withdraw_from_trade`).
- Решение Тиграна (04.11): адрес депозита BSC создаётся в кабинете с зачислением на TRADE; бот берёт именно его
  (`to = TRADE`), main → trade после депозита не нужен (`venue.deposit_to_spot` не задаётся). Адрес только MAIN при
  выключенном переводе — отказ «адрес зачисляет на MAIN» (монету не продать). Перед выводом trade → main —
  `venue.withdraw_from_trade = да`.
- «Проверить подключение» пишет тип счёта явно: Classic или UTA (`GET /api/ua/v2/account/mode`, selfAccountMode) и HF
  ли спот (`/api/v1/hf/accounts/opened`); UTA и не HF — отказ.

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки площадки, секреты (ключ, секрет, passphrase), HTTP, журнал, сырые сообщения |
| `sign.rs` | Подпись: Base64(HMAC-SHA256(timestamp+METHOD+путь?запрос+тело)), passphrase — HMAC-SHA256 секретом, KC-API-KEY-VERSION |
| `rest.rs` | REST: публичные и подписанные запросы, GET/DELETE — строка запроса (подпись до кодирования), POST — тело JSON; резерв только при сбое сети |
| `reply.rs` | Разбор ответа {code, data, msg}; 429000/HTTP 429 — пауза ресурсного пула (Public, Spot, Management) на окно 30 с |
| `errors.rs` | Коды KuCoin → единые коды (400001–400006, 400007, 411100, 429000, 200004, 115007, 260210, 500000, 230005 …) |
| `num.rs` | Числа и поля без потери точности |
| `symbols.rs`, `symbols_parse.rs` | Справочник пар `GET /api/v2/symbols`: enableTrading, аукцион, шаги baseIncrement/priceIncrement, baseMinSize/baseMaxSize, minFunds, коэффициенты комиссии |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs`, `market.rs` | Снимок пар и SpotPairs, фоновое обновление справочника, подписка (как у MEXC, решение 131) |
| `fees.rs` | Ставки комиссии `GET /api/v1/trade-fees` пачками по 10 пар × коэффициент пары |
| `wallet.rs`, `networks_parse.rs` | Монеты и сети `GET /api/v3/currencies` (разбор вне рабочих потоков, кэш за Arc), одна монета `GET /api/v3/currencies/{currency}` (решение 113), комиссия вывода |
| `deposit_addr.rs` | Адрес депозита `GET /api/v3/deposit-addresses` (кэш; memo — не поддерживается; счёт зачисления MAIN/TRADE) |
| `deposits.rs` | Статус депозита `GET /api/v1/deposits` по хешу |
| `transfers.rs` | Вывод `POST /api/v3/withdrawals` (feeDeductType INTERNAL, метка попытки в remark), статус `GET /api/v1/withdrawals/{id}`, поиск по remark |
| `fund_move.rs` | Перевод main ↔ trade (Flex Transfer) |
| `account.rs` | Балансы trade (все и одной монеты), «Проверить подключение» (версия ключа, права, без Margin/Futures, IP, HF-счёт) |
| `order_rules.rs` | Правила ордера — одни у ордера и у `check_order` (решение 134); пробный `POST /api/v1/hf/orders/test` (`venue.order_test_first`) |
| `trade.rs`, `order_state.rs` | Ордер `POST /api/v1/hf/orders/sync` limit + IOC/FOK, итог по orderId (dealSize, dealFunds, fee, feeCurrency), отмена остатка, поиск по clientOid |
| `bullet.rs` | Токен WebSocket (bullet-public / bullet-private): адрес, pingInterval, pingTimeout из ответа |
| `depth.rs` | Разбор кадров WS: welcome, pong, ack, error, снимок level2Depth5/50 |
| `ws.rs`, `ws_conn.rs`, `book.rs` | Соединения стаканов (темы по 100 символов, ≤ 400 тем), ping, стаканы в памяти и котировка |
| `user_stream.rs`, `user_stream_conn.rs`, `account_update.rs` | Поток аккаунта: /account/balance → MarketEvent::Balance, /spotMarket/tradeOrdersV2 → сырой журнал |
| `tap.rs`, `feed.rs`, `tap_decode.rs` | Запись окна (решение 125): снимки стакана, /market/match, /market/level2 + снимок REST level2_20/100 |

## Что важно знать

- WebSocket: адрес — из bullet («the URL may change»), не настройка; токен на каждое подключение; клиент шлёт ping раз
  в pingInterval; тишина дольше pingInterval + pingTimeout — обрыв. Не больше 100 символов в теме, 400 тем на соединение,
  100 сообщений клиента за 10 с (`venue.ws_subs_per_sec` ≤ 10), до 800 соединений.
- Снимок N уровней приходит только при изменении рынка («If there is no change in the market, data will not be pushed»),
  поэтому `venue.book_stale_ms` у KuCoin необязательна.
- Лимиты REST: пулы Public (по IP), Spot и Management (по UID), квота за 30 с (VIP0: 2000 / 4000 / 2000). Заголовок
  `gw-ratelimit-reset` шлюзу не виден — пул ставится на паузу на всё окно 30 с.
- Функции контракта только для DEX отвечают `NotApplicable`.
