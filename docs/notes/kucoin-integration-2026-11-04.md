# KuCoin — интеграция (решение 137) и чек-лист уроков BingX/MEXC — 2026-11-04

Ветка `router/kucoin` (от feat/own-bsc-node 85a7dcf). Адаптер — `src/venues/kucoin/` (README.md папки — ссылки и файлы).
Чек-лист — `docs/notes/cex-integration-lessons-2026-11-04.md`. Вопросы Тиграну — `docs/TASKS.md` («KuCoin»).

## Источники и версия API

- Документация https://www.kucoin.com/docs-new/ — раздел **Classic** (страницы authentication, rate-limit-rule-classic,
  error-code/spot и /http, websocket-api/base-info/introduction, bullet-public/private, страницы эндпоинтов api-34701xx).
  Страницы читались WebFetch; часть страниц по прямой ссылке закрыта — тогда факты из спецификаций ниже.
- Официальные OpenAPI-спецификации SDK **github.com/Kucoin/kucoin-universal-sdk** v1.3.2 (коммит c2f39bd, 2026-09-15):
  `spec/rest/api/openapi-spot-market.json`, `-spot-order.json`, `-account-*.json`, `spec/ws/openapi-spot-*.json` — поля,
  обязательность, перечни (`enum`), ресурсный пул (`x-api-rate-limit-pool`), вес (`x-api-rate-limit-weight`), права
  (`x-api-permission`), устаревание (`x-abandon`), темы WS (`x-topic`). Подпись — `sdk/golang/internal/infra/default_signer.go`,
  тестовый вектор — `sdk/python/.../test_default_signer.py`.
- **Classic, не UTA.** UTA — отдельный API для счёта в режиме Unified Trading Account (переход по выбору). Объявление
  «KuCoin API Upgrade (2026-06-03)»: пользователям «Pro API + Classic Account» — «Continue using the Classic API for Classic
  Account trading». Счёт бота должен оставаться классическим (вопрос 1 в TASKS).
- Живые публичные ответы 04.11 (сервер): `tests/data/cex_samples_2026_11_04/kucoin_*` — темы WS, которые техлид писал по
  памяти (`/spotMarket/level2Depth50`, `/market/level2`, `/market/match`), совпали с `x-topic` спецификации; сеть BSC в
  справочнике — chainId `bsc`, chainName `BEP20`.

## Решения адаптера (кратко)

| Что | Как | Почему |
| --- | --- | --- |
| Стакан | Снимок 5/50 уровней (`/spotMarket/level2Depth{5,50}:SYM,…`, ≤ 100 символов в теме, ≤ 400 тем на соединение) | Без REST-снимка на каждую монету (тысячи пар) и без ресинков; инкремент `/market/level2` — только в окнах 125 |
| Ордер | limit + timeInForce IOC/FOK, `POST /api/v1/hf/orders/sync`, итог — `GET /api/v1/hf/orders/{id}` | Неисполненное снимает биржа; итог несёт dealSize, dealFunds, fee, feeCurrency |
| Пробный ордер | `POST /api/v1/hf/orders/test` при `venue.order_test_first` — в ордере и в check_order | «the request parameters … are exactly the same as the order endpoint… will not enter the matching system» |
| Счета | Баланс — trade; депозит — main (или TRADE по адресу); вывод — с main; переводы — Flex Transfer | 115007; inner-transfer v2 — «abandon» |
| Лимиты | 429000/HTTP 429 → пауза пула (Public / Spot / Management) на 30 с | gw-ratelimit-reset шлюзу не виден (запрос на стык) |
| WS | Адрес и ping из bullet, новый токен на подключение | «the URL may change»; токен 24 ч |

## Уроки 1–20 → как закрыто

| № | Урок | Как закрыто (файл:строка) |
| --- | --- | --- |
| 1 | Только документация, ссылка у функции | Шапка каждого файла `src/venues/kucoin/*.rs` — раздел docs-new и/или файл спецификации SDK; README.md папки — таблица источников. Нет в документации → вопросы в docs/TASKS.md (п. 2, 5, 6, 7, 8, 9, 11) |
| 2 | Противоречия — проверять на записи | Темы WS сверены с x-topic и живой записью (`tests/kucoin_real_data.rs` ws_frames_parse). Перечень timeInForce не «разрешённые типы пары» (как orderTypes у MEXC): `order_rules.rs:38` берёт только IOC/FOK из enum, сам тип проверяет пробный ордер `order_rules.rs:139`. Права Withdrawal/FlexTransfers — из x-api-permission, не из описания permission: сверить на живом (TASKS п. 5) |
| 3 | Тесты — только записи | `tests/kucoin_real_data.rs`: 6 тестов на `kucoin_*` 04.11 и векторе SDK; приватные и /market/match — `#[ignore]` с тем, что записать (tests/README.md) |
| 4 | Одна функция `order_rules::ready` | `order_rules.rs:108`; зовут `trade.rs:29` и `mod.rs:157` (check_order); правила без сети — `order_rules.rs:54` (торгуется, IOC/FOK, шаги вниз/вверх по стороне, baseMinSize/baseMaxSize, minFunds); пробный — `order_rules.rs:139` |
| 5 | IOC — неисполненное снимает биржа | timeInForce IOC/FOK `order_rules.rs:38`; если к venue.order_final_wait_ms ордер активен — отмена `order_state.rs:50` (settle) |
| 6 | Свой ID на попытку | clientOid: метка мс + сделка, `[A-Za-z0-9_-]`, ≤ 40 — `order_rules.rs:164`, предел `:36` («maximum length … 40 characters»). У вывода clientOid нет — метка в remark `transfers.rs:33`; у перевода — 128 случайных бит `fund_move.rs` client_oid |
| 7 | Проверка факта, сроки до отправки | Факт — dealSize/dealFunds/fee/feeCurrency ордера `order_state.rs:118`; без feeCurrency при исполнении — OutcomeUnknown `:128`. Сроки venue.order_poll_ms / order_final_wait_ms проверяются в ready до отправки (`order_state::times`). Неизвестный итог постановки — поиск по clientOid, отмена по нему `order_state.rs:70` |
| 8 | Сопоставление по адресу контракта, сеть BSC — настройкой | Адрес контракта только у сети chainId = venue.bsc_network `networks_parse.rs:63`, `:102`; название сети — chainId из справочника (живое «bsc»), без значения по умолчанию |
| 9 | Большой справочник — вне рабочих потоков, Arc, одна монета | `/api/v3/currencies` (1,5 МБ): разбор в `spawn_blocking` `wallet.rs:92`, кэш за Arc (venue.networks_fresh_ms); одна монета `GET /api/v3/currencies/{currency}` — `wallet.rs:56` (coin_networks_of); справочник пар — за Arc (`symbols.rs`, all()) |
| 10 | Ввод/вывод, memo, main/funding | Флаги, withdrawalMinFee, withdrawalMinSize, maxWithdraw, withdrawPrecision, confirms, depositMinSize — `networks_parse.rs`; needTag BSC — в журнал; адрес с memo — отказ `deposit_addr.rs`; счёт зачисления MAIN/TRADE — `deposit_addr.rs`; перевод main → trade `fund_move.rs:40`, trade → main перед выводом `fund_move.rs:60`, `transfers.rs:48` |
| 11 | Комиссия по паре/ключу, монета — по факту | `GET /api/v1/trade-fees` пачками по 10 `fees.rs:32` × коэффициент пары `fees.rs:98`; монета — feeCurrency ордера |
| 12 | Стакан, лимиты подписок, пинг | Снимки 5/50 `ws.rs:29`; ≤ 100 символов в теме `ws_conn.rs:40`, ≤ 400 тем `ws_conn.rs:42`, venue.ws_symbols_per_conn ≤ 40 000 (`market.rs`), venue.ws_subs_per_sec ≤ 10 `ws.rs:27`; ping по pingInterval токена, тишина pingInterval + pingTimeout `ws_conn.rs:109`. Ресинков нет: снимок, не инкремент |
| 13 | Минимум выделений на кадр | Один разбор JSON на кадр (`depth.rs` parse), уровни сразу в Vec<PriceLevel>; один OrderBook и одна копия в память (как MEXC); сырой кадр в журнал только при raw.book_frames `ws_conn.rs:152` |
| 14 | Пересподписка не перезапускает фон | Круг комиссий перезапускается только при новом списке символов `subs.rs:121` (как решение 131 у MEXC) |
| 15 | REST-лимиты, пауза, нет REST на горячем пути | Пулы и пауза 30 с после 429000/HTTP 429 `reply.rs:29`, `:56`, `:71`; котировка — из памяти (стакан и ставки, `book.rs`), баланс — поток аккаунта и REST только в предпроверке |
| 16 | Баланс и ордера через приватный WS | bullet-private, темы `/account/balance` и `/spotMarket/tradeOrdersV2` `user_stream_conn.rs:34`; баланс → MarketEvent::Balance (`account_update.rs`); события ордеров — пока в сырой журнал (итог по REST — техдолг) |
| 17 | Готовая фича: настройки, UI, комментарии, ARCHITECTURE | Все URL/лимиты/интервалы — настройки venue.* [kucoin] без значений по умолчанию (новые: venue.withdraw_from_trade, venue.api_key_version — `settings/catalog/base.rs`, docs/SETTINGS.md); площадка на «Площадках» из реестра (`main.rs`), «Проверить подключение» `account.rs`; ARCHITECTURE «Готовые фичи», DECISIONS 137, FIRST_RUN (советы) |
| 18 | Секреты в зашифрованном файле | kucoin.api_key / api_secret / api_passphrase — `secrets/mod.rs` KNOWN, `ctx.rs` keys(); в запрос — только HMAC от passphrase `sign.rs`; токены bullet не пишутся (`reply.rs`, путь /bullet-) |
| 19 | Права ключа: только спот | Требуются General, Spot, Withdrawal (+ FlexTransfers при переводах) `account.rs:34`; Margin/Futures — отказ «Проверить подключение» `account.rs:36`; IP — venue.api_ip_bound/api_ip; белый список адресов вывода — в кабинете (в API-документации нет, TASKS п. 5) |
| 20 | Ядро и шлюз не меняются; кошелёк — «Маппинг кошельков» | Изменены только `main.rs` (регистрация), `secrets/mod.rs`, `settings/catalog/base.rs`, `window.rs` (описание), `venues/mod.rs`; строка kucoin на «Маппинге кошельков» появляется сама (биржи реестра, `api/wallets_view.rs`). Запрос на стык: заголовок gw-ratelimit-reset (TASKS п. 11) |

## Записано / не записано

- Есть (04.11, публичные): symbols v2, currencies v3, currency CAKE, part orderbook level2_20, bullet-public, WS 20 с.
- Нужно на живом боте — список в tests/README.md (kucoin_real_data.rs) и TASKS.
- Замер нагрузки при «Стопе» (решение 109) — не сделан, до включения площадки.

## Проверки

cargo fmt, cargo clippy --all-targets -D warnings — чисто; cargo test — 520 прошли, 0 упали (24 #[ignore] во всём
проекте, из них 2 — KuCoin). UI не менялся (площадки — из реестра).
