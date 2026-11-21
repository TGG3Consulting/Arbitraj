# Binance — анализ перед подключением

Claude, 05.11.2026. Только анализ, кода нет. Основа — официальная документация Binance (спот API и Wallet SAPI).
Каждый пункт сверен с чек-листом `docs/notes/cex-integration-lessons-2026-11-04.md` (уроки 1–30). Решения по
открытым вопросам принимает Тигран.

## Источники

| Метка | Источник | Что там |
|---|---|---|
| [R] | [binance-spot-api-docs / rest-api.md](https://github.com/binance/binance-spot-api-docs/blob/master/rest-api.md) (CHANGELOG «Last Updated: 2026-10-30») | Спот REST: адреса, подпись, лимиты, exchangeInfo, depth, order, order/test, account, commission |
| [WS] | [web-socket-streams.md](https://github.com/binance/binance-spot-api-docs/blob/master/web-socket-streams.md) | Публичные потоки: стакан, лимиты, пинг |
| [WA] | [web-socket-api.md](https://github.com/binance/binance-spot-api-docs/blob/master/web-socket-api.md) | WebSocket API: подписка на поток аккаунта (`userDataStream.subscribe.signature`) |
| [U] | [user-data-stream.md](https://github.com/binance/binance-spot-api-docs/blob/master/user-data-stream.md) | События потока аккаунта |
| [F] | [filters.md](https://github.com/binance/binance-spot-api-docs/blob/master/filters.md), [enums.md](https://github.com/binance/binance-spot-api-docs/blob/master/enums.md), [errors.md](https://github.com/binance/binance-spot-api-docs/blob/master/errors.md) | Фильтры пар, перечни, коды ошибок |
| [FAQ] | [faqs/commission_faq.md](https://github.com/binance/binance-spot-api-docs/blob/master/faqs/commission_faq.md), [faqs/price_range_execution_rules.md](https://github.com/binance/binance-spot-api-docs/blob/master/faqs/price_range_execution_rules.md), [faqs/api_key_types.md](https://github.com/binance/binance-spot-api-docs/blob/master/faqs/api_key_types.md) | Расчёт комиссии и скидки BNB, правило диапазона цены, типы ключей |
| [CL] | [CHANGELOG.md](https://github.com/binance/binance-spot-api-docs/blob/master/CHANGELOG.md) | Отказ от listenKey (2025-04-07, 2025-10-24), подписка по подписи |
| [W] | Wallet SAPI, страницы developers.binance.com (legacy, zh-CN): [capital](https://developers.binance.com/legacy-docs/zh-CN/wallet/capital), [withdraw](https://developers.binance.com/legacy-docs/zh-CN/wallet/capital/withdraw), [withdraw-history](https://developers.binance.com/legacy-docs/zh-CN/wallet/capital/withdraw-history), [deposite-history](https://developers.binance.com/legacy-docs/zh-CN/wallet/capital/deposite-history), [api-key-permission](https://developers.binance.com/legacy-docs/zh-CN/wallet/account/api-key-permission), [general-info](https://developers.binance.com/legacy-docs/zh-CN/wallet/general-info), [error-code](https://developers.binance.com/legacy-docs/zh-CN/wallet/error-code) | Справочник монет и сетей, вывод, депозит, права ключа, лимиты SAPI, коды −4xxx |
| [SDK] | Официальный [binance/binance-connector-rust](https://github.com/binance/binance-connector-rust) (коммит `aec7a9d`, 2026-10-29), `src/wallet/rest_api/*`; сверка — [binance-connector-js](https://github.com/binance/binance-connector-js) `clients/wallet` (коммит `62a2ec1`, 2026-10-27) | Сгенерированы из OpenAPI: пути, веса (IP/UID), параметры, поля моделей; ссылки на новые страницы `developers.binance.com/en/docs/catalog/core-trading-wallet/api/rest-api/...` |

**Что не удалось проверить:**
- Живые ответы API: `api.binance.com` из рабочей среды Claude закрыт (прокси, 403). Каждую цифру — проверить на
  записанном ответе с сервера бота (`exchangeInfo`, `capital/config/getall`, `depth`, WS-кадры).
- Новые страницы `developers.binance.com/docs/...` WebFetch не открыл (нет в разрешённых); открылись только legacy-страницы
  на китайском — цитаты из них ниже переведены. Английский текст полей Wallet — из SDK [SDK].

---

## 1. Спот API, адреса, подпись

- **Спот есть** (основной продукт). REST: `https://api.binance.com` (+ `api-gcp`, `api1…api4` — «better performance
  but less stability»); только рынок — `https://data-api.binance.vision` [R «General API Information»].
- **WebSocket рынка:** `wss://stream.binance.com:9443` или `:443`, `/ws/<stream>` или `/stream?streams=a/b/c`
  (обёртка `{"stream","data"}`); только рынок — `wss://data-stream.binance.vision` [WS «General WSS information»].
- **WebSocket API** (запросы и поток аккаунта): `wss://ws-api.binance.com:443/ws-api/v3` (порт 9443 запасной) [WA].
- **Подпись:** ключи HMAC, RSA, Ed25519 (рекомендуют Ed25519) [FAQ api_key_types]. HMAC: payload = строка запроса +
  тело **без разделителя**, не-ASCII — percent-encode до подписи; HMAC-SHA256 секретом, hex; параметр `signature` в
  query или body; заголовок `X-MBX-APIKEY`; `timestamp` (мс или мкс), `recvWindow` по умолчанию 5000, максимум 60000
  [R «Request Security», «SIGNED Endpoint Examples»]. Подпись SAPI — та же [W general-info].
- **Passphrase — нет.**
- Ответ по умолчанию JSON (SBE — по выбору, не нужен).
- Сессионная авторизация WS API (`session.logon`) — **только Ed25519** [WA «Session Authentication»]; поток аккаунта
  по подписи работает с любым типом ключа (см. п. 8).

## 2. Справочник монет и сетей

Источник: `GET /sapi/v1/capital/config/getall`, вес(IP) 10, USER_DATA (подписанный), параметров кроме `recvWindow` нет —
**весь список одним ответом, запроса одной монеты нет** [W capital], [SDK `all_coins_information`].

- **Сеть BSC:** `network: "BSC"`, `name: "BNB Smart Chain (BEP20)"` (пример в [W capital]). `venue.bsc_network` = `BSC` —
  подтвердить по живому ответу.
- **Адрес контракта:** поле сети `contractAddress` (в примере `"0xc748…e8de"` для BSC), плюс `contractAddressUrl`. Для
  нативной BNB адреса, вероятно, нет — решение 62 (WBNB по настройке). Что приходит у BNB — проверить вживую.
- **Статусы:** `depositEnable`, `withdrawEnable` по сети; `depositAllEnable` / `withdrawAllEnable` по монете;
  `depositDesc` / `withdrawDesc` — «только когда ввод/вывод закрыт»; `busy`.
- **Комиссия и пределы вывода:** `withdrawFee`, `withdrawMin`, `withdrawMax`, `withdrawInternalMin` (внутренний перевод).
- **Точность вывода:** `withdrawIntegerMultiple` (в примере `"0.01"`) — ошибка −4021 «сумма вывода должна быть кратна
  %s» [W error-code] → это шаг суммы вывода. Плюс −1111 BAD_PRECISION.
- **Минимум депозита:** поле `depositDust` (в примере `"0.01"`) — **смысл в документации не описан**. Отдельного поля
  «минимальный депозит» нет. В ответе адреса депозита (`address, coin, tag, url`) минимума тоже нет. Урок 21: «0» и
  null не путать; трактовать ли `depositDust` как минимум — вопрос Тиграну.
- **Подтверждения:** `minConfirm` — «для зачисления», `unLockConfirm` — «для разблокировки» [W capital].
- **Memo/tag:** `withdrawTag` — «нужен ли memo при выводе» (`sameAddress` устарел); `memoRegex`, `addressRegex`. Для
  сети без memo непустой `addressTag` → −4106 [W withdraw]. BSC в примере `withdrawTag: false`.
- **`denomination`** — у «1M x …» монет (1 1MBABYDOGE = 1 000 000 BABYDOGE). Сопоставление по адресу контракта это
  разрулит, но кол-во на бирже и в пуле — в разных единицах. Такие монеты — отсеивать или учитывать множитель (вопрос).
- Справочник пар: `GET /api/v3/exchangeInfo` (вес 20); фильтр `permissions=SPOT`, `symbolStatus=TRADING` [R].

## 3. Вывод и депозит через API

**Вывод:** `POST /sapi/v1/capital/withdraw/apply`, вес(UID) **900** [W withdraw], [SDK].
- Параметры: `coin`, `network`, `address`, `amount`, `withdrawOrderId` (свой id, «можно искать в withdraw/history»),
  `addressTag`, `transactionFeeFlag` (только для внутреннего перевода), `name` (добавит адрес в адресную книгу, предел
  200), `walletType` — **0 спот, 1 фонд; по умолчанию — счёт, выбранный в кабинете для депозита**. Ответ: `{"id"}`.
- **Правило путешествия (travel rule):** проверить `GET /sapi/v1/localentity/questionnaire-requirements`; если не `NIL` —
  нужен другой эндпоинт `POST /sapi/v1/localentity/withdraw/apply` [W withdraw]. Зависит от юрлица Binance, к которому
  привязан аккаунт Тиграна — **проверить живым запросом** (в «Проверить подключение»).
- **Белый список:** −4035 «адрес не в белом списке» (если в кабинете включён белый список), −4036 «новый адрес — вывод
  через {0} ч» [W error-code]. Адресная книга: `GET /sapi/v1/capital/withdraw/address/list` (поле `whiteStatus`) [SDK].
  Нужно заранее добавить адрес кошелька DEX (сеть BSC) и показывать в «Проверить подключение».
- **Права ключа:** `enableWithdrawals`; «включение вывода требует ограничения ключа по IP» [W api-key-permission].
- **Статусы вывода** (`withdraw/history`, параметр `status`): 0 — отправлено письмо, 2 — ждёт подтверждения,
  3 — отклонён, 4 — в обработке, 6 — завершён [W withdraw-history], то же в [SDK]. **Значения 1 и 5 в документации не
  перечислены** — что значат, если придут, — проверить вживую / вопрос Тиграну. `info` — «причина неудачи вывода».
- **Поиск по своему id:** `withdraw/history?withdrawOrderId=` — окно ≤ 7 дней (без дат — последние 7 дней); или
  `idList` (до 45 id). **Вес(UID) 18000, «10 запросов в секунду»** [W withdraw-history]. При UID-пределе SAPI 180000 в
  минуту на один эндпоинт [W general-info] это **~10 запросов в минуту** — опрос статуса вывода не чаще раза в 6 с на
  всё (настройка). Хеш — `txId`, факт комиссии — `transactionFee`, `walletType`, `completeTime`.

**Депозит:**
- Адрес: `GET /sapi/v1/capital/deposit/address?coin=&network=`, вес(IP) 10, ответ `address, coin, tag, url` [SDK].
  Кэш по (монета, сеть), запрос в предпроверке (урок 25).
- История: `GET /sapi/v1/capital/deposit/hisrec`, вес(IP) 1, **есть параметр `txId`** — поиск по хешу [W deposite-history].
- Статусы: 0 — ожидает, **6 — «зачислено, вывод заблокирован»** (credited but cannot withdraw), 7 — ошибочный депозит,
  8 — ждёт подтверждения пользователя, 1 — успех, 2 — отклонён [W deposite-history], [SDK]. 6 = аналог MEXC
  PRE_SUCCESS (урок 22): торговать можно, выводить нельзя (ошибка вывода −4060 «депозит в подтверждении, часть
  активов заблокирована»). Плюс `travelRuleStatus` (1 — нужно заполнить данные депозита) — не торговать до 0.
- **Счёт зачисления:** в записи депозита `walletType` (0 спот, 1 фонд). Куда идёт депозит — настройка кабинета
  («Кошелёк → Спот/Фонд → Депозит», см. описание `walletType` у вывода). Если фонд — перевод
  `POST /sapi/v1/asset/transfer` type `FUNDING_MAIN`, вес(UID) 300, право `permitsUniversalTransfer` [SDK
  `user_universal_transfer`]. Как BingX `venue.deposit_to_spot`. Проще — выставить в кабинете зачисление на спот, а
  бот проверяет `walletType` и при 1 переводит (или отказ с причиной).

## 4. Ордера

- `POST /api/v3/order`, вес 1, источник Matching Engine [R «New order»].
- **IOC — параметр `timeInForce=IOC`** у `type=LIMIT` (обязательны `timeInForce, quantity, price`); есть FOK [F enums
  «Time in force»]. Неисполненный остаток снимает биржа: статус `EXPIRED`, `expiryReason`
  `UNFILLED_IOC_QUANTITY_EXPIRED`. `orderTypes` в exchangeInfo — перечень типов (`LIMIT` и др.), timeInForce там не
  перечисляется — урок 30: проверить пробным ордером.
- **Свой id:** `newClientOrderId` — «уникален среди открытых ордеров». Длина и алфавит в спот-документации **не указаны**
  (есть только общий −1100 «Illegal characters … legal range is '%s'»). Новый на каждую попытку (урок 6); формат —
  взять из ответа −1100 на живом/пробном ордере.
- **Итог в ответе:** для LIMIT по умолчанию `newOrderRespType=FULL`: `status`, `executedQty`, `cummulativeQuoteQty`
  (средняя = cummulativeQuoteQty / executedQty), `fills[]` с `price, qty, commission, commissionAsset` — итог сразу в
  ответе, без второго запроса (в отличие от MEXC). Запрос: `GET /api/v3/order` (вес 4) по `orderId` или
  `origClientOrderId`; сделки — `GET /api/v3/myTrades`.
- **Неизвестный исход:** 5XX — «execution status is UNKNOWN»; −1007 TIMEOUT (10 с) — «Send status unknown» [R] → поиск
  по `origClientOrderId`.
- **Правила пары (фильтры)** [F filters]: `PRICE_FILTER` (`minPrice/maxPrice/tickSize`, 0 — выключено), `LOT_SIZE`
  (`minQty/maxQty/stepSize`), `NOTIONAL` (`minNotional/maxNotional`) / `MIN_NOTIONAL`, `PERCENT_PRICE` /
  `PERCENT_PRICE_BY_SIDE` (цена ордера относительно средней или **референсной цены** × множители — наша «худшая цена»
  IOC может не пройти), `MAX_NUM_ORDERS`, `MAX_POSITION`. Плюс **правило исполнения `PRICE_RANGE`**
  (`GET /api/v3/executionRules`, `GET /api/v3/referencePrice`, поток `<symbol>@referencePrice`): сделка вне диапазона →
  `EXPIRED` с `EXECUTION_RULE_PRICE_RANGE_EXCEEDED` [FAQ price_range_execution_rules].
- **Статус пары:** `status` (TRADING / END_OF_DAY / HALT / BREAK / CANCEL_ONLY), `isSpotTradingAllowed`,
  `permissionSets` (ордер разрешён, если права **аккаунта** покрывают набор: `[["A","B"]]` — A или B) [R exchangeInfo].
  Права аккаунта — `permissions` в `GET /api/v3/account`. Это аналог MEXC selfSymbols: монета торгуется, только если
  набор прав пары покрыт правами аккаунта (иначе точная причина в SpotPairs).
- **Пробный ордер есть:** `POST /api/v3/order/test` — «validates … but does not send it into the matching engine», вес 1;
  с `computeCommissionRates=true` (вес 20) — ставки именно для этого ордера [R «Test new order»]. Для `check_order` и
  `venue.order_test_first`.

## 5. Стакан

- **Частичный стакан:** `<symbol>@depth<5|10|20>` или `…@100ms`, 1000 мс или 100 мс, полный снимок `lastUpdateId, bids,
  asks` [WS «Partial Book Depth Streams»]. В кадре **нет символа** — нужен комбинированный поток (`stream` в обёртке).
  Шлётся по таймеру, не только при изменении → отдавать ядру только при изменении `lastUpdateId` (решение 109).
  Максимум 20 уровней — проверить, хватает ли глубины для наших сумм.
- **Дифф:** `<symbol>@depth` / `@depth@100ms`, `U`/`u`; снимок REST `GET /api/v3/depth` (limit до 5000; вес 5 / 25 / 50 /
  250 по limit) и процедура сборки: `lastUpdateId < U` → повторить снимок; `U > local+1` → пересобрать
  [WS «How to manage a local order book correctly»]. Ресинк многих пар со снимком 5000 (вес 250) быстро съест
  REST-вес — снимок брать малым `limit` (настройка).
- **Лимиты WS:** 5 входящих сообщений/с на соединение (ping, pong, subscribe…), до **1024 потоков на соединение**,
  **300 подключений за 5 мин с IP**, соединение живёт 24 ч, событие `serverShutdown` [WS «WebSocket Limits»].
  `id` запроса — число, строка до 36 или null.
- **Пинг:** сервер шлёт ping каждые 20 с, pong (с тем же payload) — не позже минуты, иначе обрыв [WS].
- Символы в потоках — **строчные**.

## 6. Комиссии

- По паре и аккаунту: `GET /api/v3/account/commission?symbol=` (вес 20) — `standardCommission`, `specialCommission`,
  `taxCommission` (maker/taker/buyer/seller), `discount` {`enabledForAccount`, `enabledForSymbol`, `discountAsset`:
  BNB, `discount` — множитель} [R «Query Commission Rates»]. Комиссия = сумма × (taker + buyer/seller) по каждому
  виду; покупка — с полученного количества монеты, продажа — с полученных USDT [FAQ commission].
- Все пары сразу: `GET /sapi/v1/asset/tradeFee` (вес(IP) 1, `makerCommission/takerCommission`) [SDK] — без
  special/tax; для расчёта брать `account/commission` фоном с паузами (вес 20 × N пар) или `order/test` с
  `computeCommissionRates`.
- Общие ставки аккаунта: `commissionRates` в `GET /api/v3/account`.
- **Скидка BNB:** включается `POST /sapi/v1/bnbBurn` (`spotBNBBurn`), статус — `GET /sapi/v1/bnbBurn` [SDK wallet,
  margin_trading]. Отражается в `discount.enabledForAccount`. Скидка — только на standard (не на tax/special); **при
  нехватке BNB полная комиссия берётся из полученной монеты** (в FAQ это сказано прямо — у Gate было неизвестно).
  Урок 23: расчёт по обычной ставке, факт и монета — из `fills[].commissionAsset`.
- Расхождение в примерах: в `account/commission` комментарий «0.75 = 75% of the standard commission is charged», в FAQ
  пример `0.25` — оба трактуют поле как **множитель**; значения в примерах вымышленные. Проверить на живом ответе.

## 7. Лимиты запросов

- `/api/*` — **по IP, не по ключу**: `REQUEST_WEIGHT`, `RAW_REQUESTS`, `ORDERS` из `exchangeInfo.rateLimits`
  (в примере 6000 веса/мин, ордера 50/10 с и 160000/сутки, 61000 запросов/5 мин) — **брать из живого ответа**.
  Заголовки `X-MBX-USED-WEIGHT-1M`, `X-MBX-ORDER-COUNT-*` [R «LIMITS»].
- Превышение: **429 + `Retry-After` (секунды)**; продолжение после 429 → **418 бан IP от 2 мин до 3 суток**; 403 — WAF.
  Ордера: «Unfilled Order Count» по аккаунту (IOC с исполнением не считаются) [R], [faqs/order_count_decrement.md].
- `/sapi/*` — свой режим: **предел на каждый эндпоинт отдельно**, IP 12000/мин, UID 180000/мин; заголовки
  `X-SAPI-USED-IP-WEIGHT-1M` / `-UID-`; при превышении только 429, бан 418 к SAPI не применяется; есть ли Retry-After
  у SAPI — прямо не сказано [W general-info] → пауза по Retry-After, если пришёл, иначе `venue.rate_limit_pause_ms`.
- Тяжёлые: `withdraw/history` 18000 UID, `withdraw/apply` 900 UID, `asset/transfer` 300 UID, `exchangeInfo` 20,
  `account` 20.

## 8. Приватный WS (поток аккаунта)

- **listenKey устарел:** с 2025-04-07 «deprecated», с 2025-10-24 документация удалена, «remain available until a future
  retirement announcement» [CL]. Новый путь — **WebSocket API**: `userDataStream.subscribe.signature` с `apiKey`,
  `timestamp`, `signature` — без логина, **любой тип ключа (HMAC тоже)** [WA], [CL]. (`userDataStream.subscribe` —
  только с `session.logon`, Ed25519.) Ответ `subscriptionId`; события приходят как `{"subscriptionId", "event"}`.
- События [U]: `outboundAccountPosition` (балансы free/locked изменённых монет), `balanceUpdate` (депозит/вывод/перевод,
  дельта, **без хеша**), `executionReport` (ордер: `z`, `Z`, `n`, `N`, `X`, `eR`; «средняя = Z / z»),
  `eventStreamTerminated`, `externalLockUpdate`.
- Соединение WS API живёт 24 ч, пинг 20 с — как у потоков.

## 9. Счета (панель «Балансы»)

| Счёт | Эндпоинт | Источник |
|---|---|---|
| Спот | `GET /api/v3/account` (вес 20, `omitZeroBalances`) | [R] |
| Фонд (Funding) | `POST /sapi/v1/asset/get-funding-asset` (вес 1). В заметке SDK: «Currently supports querying the following business assets: Binance Pay, Binance Card, Binance Gift Card, Stock Token» — **полнота по крипте неясна, проверить вживую** | [SDK] |
| Все кошельки сводно | `GET /sapi/v1/asset/wallet/balance` (вес 60; `walletName`, `balance` в `quoteAsset`) — Spot, Funding и др. | [SDK] |
| Earn (Simple Earn) | `GET /sapi/v1/simple-earn/account` (вес 150) | [SDK simple_earn] |
| Маржа, фьючерсы | Только чтение для панели: margin SAPI / `fapi`, `dapi` — отдельные API; торговли нет | — |

Урок 24: ошибка каждого счёта — в его `error`; «счёт не открыт» (урок 27) — пустой счёт, коды — по живому ответу.

## 10. Права ключа

- Проверка: `GET /sapi/v1/account/apiRestrictions` (вес 1): `ipRestrict`, `enableReading`, `enableWithdrawals`
  («включение требует IP-ограничения»), `enableSpotAndMarginTrading`, `permitsUniversalTransfer`,
  `enableInternalTransfer`, `enableMargin`, `enableFutures`, `enableVanillaOptions`, `enablePortfolioMarginTrading`,
  FIX [W api-key-permission].
- Нужны: `enableReading`, `enableSpotAndMarginTrading` (спот и маржа одним флагом — маржу отдельно не выключить),
  `enableWithdrawals` (+ `ipRestrict = true`, IP сервера), `permitsUniversalTransfer` — если депозит идёт на Фонд.
  Фьючерсы/опционы/PM — не нужны (у Тиграна ключи с полными доступами — показывать, не блокировать; решение Тиграна).
- Дополнительно: `GET /api/v3/account` — `canTrade`, `canWithdraw`, `canDeposit`, `permissions`; `GET
  /sapi/v1/account/apiTradingStatus` — блокировка API-торговли (`isLocked`, триггеры) [SDK].
- По умолчанию ключ не может TRADE — включается в кабинете [R «Request Security»].

## 11. Уроки 1–30 — что важно у Binance

| # | Урок | Binance |
|---|---|---|
| 1 | Только документация | Есть полная: GitHub [R]/[WS]/[WA] + Wallet SAPI [W]/[SDK]. Новые страницы docs из среды Claude не открылись |
| 2 | Противоречия → живой ответ | Множитель `discount` (0.75 vs 0.25); статусы вывода 1/5 отсутствуют; смысл `depositDust`; `denomination` |
| 3 | Тесты на записях | Публичные (exchangeInfo, depth, WS) — записать с сервера; getall и приватные — `#[ignore]` |
| 4 | Одна `order_rules::ready` + пробный | Фильтры явные (tickSize/stepSize/notional) — проще MEXC; `order/test` есть; добавить проверку PERCENT_PRICE_BY_SIDE по референсной цене, если проверяема |
| 5 | IOC | `timeInForce=IOC` — родной, остаток снимает биржа (`EXPIRED`) |
| 6 | Новый id на попытку | `newClientOrderId`; формат — из ответа −1100 (в доках нет) |
| 7 | Итог ордера | В ответе FULL: executedQty, cummulativeQuoteQty, fills с commission/commissionAsset |
| 8 | Сопоставление по адресу | `contractAddress` в сети `BSC` |
| 9 | Огромный справочник | getall — весь список (одной монеты нет), exchangeInfo — тысячи пар: `spawn_blocking`, Arc, без пересборки при том же содержимом |
| 10 | Ввод/вывод, счёт зачисления | Статусы, `withdrawFee/Min/Max`, `withdrawIntegerMultiple`, `withdrawTag`; депозит на Спот или Фонд по настройке кабинета → `walletType` + `FUNDING_MAIN` |
| 11 | Комиссия из API | `account/commission` по паре (+ tax/special) |
| 12 | Стакан, лимиты подписок | `@depth20@100ms` или дифф + снимок; 1024 потока/соединение, 5 сообщений/с, 300 подключений/5 мин; ping 20 с |
| 13 | Минимум выделений | JSON-кадры (не protobuf); частичный стакан шлётся по таймеру — отсев по `lastUpdateId` до разбора уровней |
| 14 | Пересподписка | Как у MEXC (решение 131) |
| 15 | REST-лимиты | По IP (не ключу); 429 + Retry-After; **418 бан** — шлюз обязан останавливаться; SAPI — по эндпоинту |
| 16 | Приватный WS | WS API `userDataStream.subscribe.signature` (не listenKey) |
| 17 | Готовая фича | Стандартно; новые настройки: `venue.ws_api_url`, `venue.sapi_*` паузы |
| 18 | Секреты | Ключ + секрет (HMAC) или приватный ключ Ed25519; passphrase нет |
| 19 | Права ключа | apiRestrictions; вывод только с IP-ограничением; белый список / адресная книга |
| 20 | Ядро не трогаем | Подходит |
| 21 | Минимум депозита | Явного нет; `depositDust` — смысл не описан → вопрос, `venue.deposit_min_absent_ok` |
| 22 | Удержание вывода | **Есть**: депозит статус 6 «credited but cannot withdraw», ошибка −4060, `unLockConfirm` → `withdraw_hold` |
| 23 | Скидка своей монетой | BNB: `bnbBurn`, `discount` в API; при нехватке BNB — полная комиссия в монете (документировано) |
| 24 | Все счета | См. п. 9 |
| 25 | Адрес депозита — кэш | `capital/deposit/address` |
| 26 | Повтор круга комиссий | `venue.fee_retry_s` |
| 27 | «Счёт не открыт» | Коды для Earn/фьючерсов — по живому ответу |
| 28 | gzip, IPv4/IPv6 | Сжатие ответа — по живому заголовку; `venue.api_ip` при необходимости |
| 29 | Права — своя запись | apiRestrictions — права **текущего** ключа; имена прав — из [W] |
| 30 | IOC не в списке типов | `orderTypes` содержит типы, не timeInForce → проверить IOC пробным ордером |

Ещё, вне 30 уроков:
- **Опрос статуса вывода дорогой** (18000 UID, ~10/мин) — частота опроса — настройка; будить опрос событием
  `balanceUpdate`.
- **Travel rule** — зависит от юрлица аккаунта; если требуется — другой эндпоинт вывода и анкеты депозита
  (`travelRuleStatus = 1` блокирует средства).
- **Новый адрес вывода — задержка** (−4036) — добавить адрес заранее.
- **Монеты «1M x …»** с `denomination` — единицы на бирже и в пуле разные.
- Доступ к Binance с сервера (Франция, OVH) и из страны аккаунта — проверить первым живым запросом.

## 12. Вердикт

**ПОДКЛЮЧАТЬ.** Спот есть, сеть BSC есть (`BSC`, «BNB Smart Chain (BEP20)»), адрес контракта в справочнике есть
(`contractAddress`), вывод и депозит через API есть (свой id вывода, поиск депозита по хешу), IOC родной, пробный ордер
есть, итог ордера с комиссией в ответе, поток аккаунта по WS.

**Сложность — низкая/средняя.** Самый близкий образец — **MEXC** (MEXC Spot V3 — копия Binance API: те же
`exchangeInfo`, фильтры, `order/test`, HMAC «query+body», `capital/config/getall`), но у Binance проще: стакан в JSON
(не protobuf), явные `tickSize/stepSize`, `timeInForce=IOC`, итог в ответе на ордер. Новое по сравнению с MEXC:
поток аккаунта через WS API (не listenKey), Wallet под `/sapi` со своими лимитами, депозит на Спот или Фонд, статус 6
(удержание), travel rule, PRICE_RANGE.

Вопросы Тиграну (перед кодом):
1. Депозит в кабинете Binance — на Спот или Фонд? (Если Фонд — нужен `permitsUniversalTransfer` и перевод.)
2. `depositDust` считать минимумом депозита? Или `venue.deposit_min_absent_ok`?
3. Статусы вывода 1 и 5 (нет в документации) — если придут, считать «ждём» и уведомлять?
4. Тип ключа: HMAC (как у остальных бирж) или Ed25519 (рекомендация Binance, нужна новая зависимость)?
5. Стакан: `@depth20@100ms` (просто) или дифф + снимок (глубже, дороже по весу)?
6. Монеты с `denomination` — отсеивать?
