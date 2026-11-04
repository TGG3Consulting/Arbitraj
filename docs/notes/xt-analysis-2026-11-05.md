# XT.com — анализ для подключения (05.11.2026, без кода)

Задача: можно ли подключить XT.com как биржу направления «биржа ↔ DEX (BSC)» и «биржа ↔ биржа» по правилам CLAUDE.md
и чек-листу `docs/notes/cex-integration-lessons-2026-11-04.md` (30 уроков).

## Источники и что проверить не удалось

| Источник | Что это |
| --- | --- |
| https://doc.xt.com/ | Официальная документация API v4 (одна страница, разделы по якорям `#<раздел><файл>`). Инструментом напрямую не открылась (прокси 403 / ограничение источника) |
| https://xt-com.github.io/xt4-api/ | Та же документация на GitHub Pages (открылась, содержимое совпадает) |
| https://github.com/xt-com/xt4-api (ветка main, коммит `8edf738` от 22.01.2025) | **Исходник doc.xt.com** (файл `CNAME` = `doc.xt.com`): разделы в `_documentation/`, `_market/`, `_balance/`, `_deposit_withdrawal/`, `_order/`, `_trade/`, `_transfer/`, `_websocket_public/`, `_websocket_private/`, `_user_center_apikey/`, `_futures_*`, `_margin_balance/` |
| https://github.com/kelvinxue/pyxt (коммит `db336a1` от 20.11.2024) | Python SDK, на который ссылается сама документация («API Resources and Support» → Python SDK) — список спот-эндпоинтов совпадает с документацией |

Ниже ссылки вида `doc.xt.com/#marketsymbol` — якорь раздела на сайте; в скобках — файл исходника в `xt-com/xt4-api`.

**Не удалось проверить:** живые ответы API (`sapi.xt.com` из среды недоступен — прокси отклоняет соединение).
Название сети BSC, наличие поля `contract` у реальных монет, типы чисел в ответах, IPv4/IPv6 — только на живой записи.
Последний коммит документации — январь 2025: свежесть надо сверить с живым сайтом и ответами API.

## 1. Спот API, адреса, подпись

- Спот есть: REST `https://sapi.xt.com` (doc.xt.com/#documentationrestApi, `_documentation/restApi.md`), пути `/v4/...`.
- WebSocket публичный `wss://stream.xt.com/public` (`_websocket_public/base.md`), приватный `wss://stream.xt.com/private`
  (`_websocket_private/base.md`). Обязателен заголовок `Sec-Websocket-Extensions: permessage-deflate` (оба).
- Формат ответа REST: `{rc, mc, ma, result}`; `rc` 0 — успех, 1 — отказ; `mc` — код (`_documentation/returnFormat.md`,
  `errorCode.md`).
- Подпись спота (doc.xt.com/#documentationsignStatement, #documentationsignSteps; `signStatement.md`, `signSteps.md`):
  - заголовки `validate-algorithms` (HmacSHA256 рекомендуется; есть MD5/SHA1/SHA224/SHA384/SHA512), `validate-appkey`,
    `validate-recvwindow` (мс), `validate-timestamp` (мс), `validate-signature` (hex); `validate-signversion` — резерв;
  - X = заголовки `validate-algorithms, validate-appkey, validate-recvwindow, validate-timestamp` по алфавиту,
    `key=value` через `&`;
  - Y = `#METHOD#path#query#body` (query — ключи по алфавиту; JSON-тело — как есть, без сортировки; пустая часть пропускается);
  - `signature = HMAC_SHA256_hex(secret, X + Y)`.
  - Пример строки с реальной подписью есть в документации — пригоден как тестовый вектор.
- Passphrase **нет**. Подписываются все пути, кроме `/public` (`apiBasicInfo.md`). Content-Type `application/json`.
- recvWindow: текст противоречив («between 2 to 60 seconds», «older than 5,000 ms is invalid», «longer than 5 seconds is
  not recommended»), пример — 60000. Часы клиента впереди сервера больше 1 с — отказ; просрочка — `AUTH_105` (FAQ.md).
  → настройка `venue.recv_window_ms`, значение подтверждает Тигран.
- **Фьючерсы подписываются иначе** (`_futures_documentation/signSteps.md`): X = только `validate-appkey`, `validate-timestamp`;
  Y = `#path#query#body` без метода; хосты `fapi.xt.com` / `dapi.xt.com`. Нужно только для панели «Балансы».

## 2. Справочник монет и сетей

- Монеты и сети: `GET /v4/public/wallet/support/currency` (doc.xt.com/#deposit_withdrawalsupportedCurrenciesGet,
  `_deposit_withdrawal/supportedCurrenciesGet.md`): `currency`, `supportChains[]` с полями `chain`, `depositEnabled`,
  `withdrawEnabled`, `withdrawFeeAmount`, `withdrawMinAmount`, `depositFeeRate`, **`contract`** (адрес контракта).
  «The currency and chain in the response need to be used in other deposit/withdrawal API».
- **Адрес контракта есть** — поле `contract` у каждой сети. Пустое ли оно у нативных монет и в каком регистре — проверить на
  живой записи (урок 8).
- **Название сети BSC в документации не указано** (в примерах только `Bitcon`, `Ethereum`, `Tron`, `XLM`). Брать из живого
  справочника → `venue.bsc_network`.
- Статус ввода/вывода — по сети (`depositEnabled`/`withdrawEnabled`) и по монете (`GET /v4/public/currencies`:
  `depositStatus`, `withdrawStatus` 0/1, `maxPrecision`; doc.xt.com/#balancecurrenciesGet, `_balance/currenciesGet.md`).
- Комиссия вывода — `withdrawFeeAmount` (фиксированная, в монете), минимум — `withdrawMinAmount`.
- **Комиссия депозита `depositFeeRate`** — у XT есть ставка за ввод. Это дополнительная затрата стороны продажи; её
  смысл (доля? процент?) в документации не описан → вопрос Тиграну, проверка на живой записи.
- Точность вывода по сети — **нет в документации**; есть только `maxPrecision` монеты и ошибка `WITHDRAW_010` «precision of
  withdrawal is invalid».
- Минимум депозита — **нет в документации** (ни в справочнике, ни в ответе адреса депозита) → урок 21:
  `venue.deposit_min_absent_ok` — решение Тиграна.
- Требуемое число подтверждений депозита — **нет в документации** (в истории депозитов есть только текущее `confirmations`).
- memo/tag: адрес депозита отдаёт `memo` (`depositAddressGet.md`); вывод принимает `memo` (WITHDRAW_006 «Memo cannot be
  empty»). Для BSC ожидается пустой memo; непустой — отказ, как у Gate/KuCoin.
- Одной монеты запросом нет — справочник только целиком (параметров нет). Размер — на живой записи (урок 9: разбор вне
  потоков tokio, кэш за Arc).
- Справочник пар `GET /v4/public/symbol` (doc.xt.com/#marketsymbol, `_market/2symbol.md`) умеет `symbol`, `symbols` и
  `version`: при совпадении версии список не возвращается — удобно для решения 131 (не пересобирать ядро без изменений).

## 3. Вывод и депозит через API

**Вывод** — `POST /v4/withdraw` (doc.xt.com/#deposit_withdrawalwithdraw, `withdraw.md`), тело JSON: `currency`, `chain`,
`amount` (**«including handling fee»** — сумма включает комиссию), `address`, `memo`, `clientOrderId`
(`^[a-zA-Z0-9_]{4,32}$`), `toAccountId` (внутренний перевод пользователю XT). Ответ — `result.id`. Лимит 1/с на ключ.
- Белый список адресов: явного описания нет; есть код `WITHDRAW_004` «The withdrawal address is not added» и
  `WITHDRAW_016` «address already exists» → вероятно, адрес нужно заранее добавить в адресную книгу в кабинете.
  **Не додумываем — вопрос Тиграну / проверка на живом выводе.**
- Право ключа — `WITHDRAW` (см. п. 10).
- Статус по своему id: `GET /v4/withdraw?recordId=|clientOrderId=` (doc.xt.com/#deposit_withdrawalwithdrawDetail,
  `withdrawDetail.md`; лимит 1/с на ключ): `status`, `amount`, `fee`, `chain`, `transactionId`, `confirmations`.
  История — `GET /v4/withdraw/history` (`currency` и `chain` обязательны, до 200 записей).
- Статусы (doc.xt.com/#documentationpublicModule, `publicModule.md`): SUBMIT, REVIEW, AUDITED, AUDITED_AGAIN, PENDING,
  SUCCESS, FAIL, CANCEL. REVIEW — ручная проверка возможна (время в пути непредсказуемо).
- Противоречие: поле `type` описано как CHAIN_TRANSFER/INTERNAL_TRANSFER, а в примере — число `0`; `clientOrderId` в
  примере — число `10`. → разбор по живой записи.

**Депозит**
- Адрес: `GET /v4/deposit/address?chain=&currency=` → `address`, `memo` (doc.xt.com/#deposit_withdrawaldepositAddressGet).
  Коды: `DEPOSIT_006` нет адреса, `DEPOSIT_007` адрес генерируется — повтор позже (урок 25: кэш, запрос в предпроверке).
- История: `GET /v4/deposit/history` (`currency`, `chain` обязательны; `status`, `fromId`, `direction`, `limit` ≤ 200,
  `startTime`, `endTime`) → `id`, `status`, `amount`, `confirmations`, **`transactionId`**, `address`, `fromAddr`.
  **Фильтра по хешу нет** — искать `transactionId` в странице за окно времени (окно и размер — настройки, урок 7).
- Статусы депозита — те же; «предварительно зачислено» (как MEXC PRE_SUCCESS) — **нет в документации**.
- Есть ошибка вывода `WITHDRAW_009` «some assets in this withdraw are restricted by T+1 withdraw» — **удержание вывода
  T+1 существует**, но как его узнать заранее через API — **нет в документации** (урок 22, `withdraw_hold`). Вопрос Тиграну.
- На какой счёт зачисляется депозит — **нет в документации**. Внутренний перевод есть: `POST /v4/balance/transfer`
  (`bizId` для идемпотентности, `from`/`to` из SPOT, LEVER, FINANCE, FUTURES_U, FUTURES_C; doc.xt.com/#transfertransferPost).
  Скорее всего зачисление на SPOT — подтвердить первым живым депозитом.

## 4. Ордера

- `POST /v4/order` (doc.xt.com/#orderorderPost, `_order/orderPost.md`): `symbol`, `side` BUY/SELL, `type` LIMIT/MARKET,
  `timeInForce` **обязателен**: GTC, **IOC**, FOK, GTX; `bizType` SPOT (обязателен); `price`, `quantity`, `quoteQty`;
  `clientOrderId` `^[a-zA-Z0-9_]{4,32}$`. Лимит 20/с на ключ. Ответ — только `orderId` (итог — отдельным запросом).
- IOC есть — стартовое правило «не исполнился — отменяем» закрывается типом (урок 5). Состояние IOC с остатком
  (CANCELED или EXPIRED) — в документации не сказано → проверить на живом ордере (урок 30). У пары есть список
  `timeInForces` — сверять, но доверять живому ответу (урок 2).
- Уникальность `clientOrderId` в документации не описана — новый на каждую попытку (урок 6).
- Итог: `GET /v4/order/{orderId}` или `GET /v4/order?orderId=|clientOrderId=` (#orderorderGet, #orderorderGetQueryParam):
  `state`, `executedQty`, `leavingQty`, `avgPrice`, `tradeBase`, `tradeQuote`, `fee`, `feeCurrency`,
  **`deductServices[]`** (если включено списание комиссии в XT — комиссия в этом списке, а не в `fee`). Сделки —
  `GET /v4/trade?orderId=` (`fee`, `feeCurrency`, `takerMaker`). Лимиты на эти GET — не указаны.
- Правила пары (`/v4/public/symbol`): `state` ONLINE/OFFLINE/DELISTED, `tradingEnabled`, **`openapiEnabled`** (торговля через
  API), `pricePrecision`, `quantityPrecision`, фильтры `PRICE` (min/max/tickSize), `QUANTITY` (min/max/tickSize),
  `QUOTE_QTY` (min), `PROTECTION_LIMIT` (отклонение от последней цены сделки), `PROTECTION_MARKET`, `PROTECTION_ONLINE`.
  **null = без ограничения** (документация явно). Шаг цены: `(price − minPrice) % tickSize == 0` — от min, не от нуля.
- `PROTECTION_LIMIT` зависит от последней цены сделки — проверяем в `order_rules::ready`, только если есть свежая цена
  сделки из `trade@{symbol}`; иначе — проверка невозможна, решает Тигран (урок 4).
- Пробного ордера (test order) **нет в документации** (и в SDK) → `venue.order_test_first` = да для XT — ошибка настройки.
- Ошибки ордера подробные: ORDER_F0101…F0601 по каждому фильтру, `SYMBOL_005` «does not support trading via API»,
  **`SYMBOL_011`** «high-risk… trading restricted… contact customer service for approval» — пары повышенного риска
  закрыты до одобрения поддержкой.

## 5. Стакан

- `depth@{symbol},{levels}` — снимок 5/10/20/50 уровней, **раз в 1000 мс** (#websocket_publiclimitDepth). Для арбитража
  медленно.
- `depth_update@{symbol}` — изменения раз в **100 мс**, поля `fi` (first = прошлый `i` + 1), `i` (last)
  (#websocket_publicincreDepth). Локальный стакан — по инструкции «Orderbook manage» (#websocket_publicorderBook), копия
  Binance: буфер → REST-снимок `/v4/public/depth?limit=500` (`lastUpdateId`) → отбросить `i ≤ lastUpdateId` → первый
  `fi ≤ lastUpdateId+1 ≤ i` → далее `fi == прошлый i + 1`; 0 — удалить уровень. Разрыв → пересинхронизация.
- REST-снимок: `GET /v4/public/depth` (`limit` 1–500), **10/с на IP** — старт и каждый ресинк на сотни пар упираются в этот
  лимит (очередь снимков с паузой — настройка).
- Подписка: `{"method":"subscribe","params":[...],"id":"..."}`, ответ `{id, code, msg}`.
- Пинг: клиент шлёт текст `ping`, сервер — `pong`; без ping 1 мин — разрыв (#websocket_publicheartBeat).
- **Лимиты подписок на соединение, тем в одном запросе, сообщений в секунду, соединений с IP — нет в документации**
  → вопрос Тиграну / подбор на живом замере (`venue.ws_symbols_per_conn`, `venue.ws_subs_per_sec`).

## 6. Комиссии

- В справочнике пары есть `takerFeeRate` / `makerFeeRate` — ставка пары, не ключа.
- **Эндпоинта ставки комиссии по аккаунту/ключу в спот API v4 нет** (нет и в SDK). Урок 11 закрыть нельзя «по ключу»:
  остаётся ставка пары из справочника + факт из ордера. Вопрос Тиграну: достаточно ли `takerFeeRate` пары (VIP-уровни
  тогда не видны).
- Скидка своей монетой XT: в ордере `deductServices` с `feeCurrency: "xt"` («if set XT deduction fee»). Как включить и как
  прочитать, включена ли, — **нет в документации** (кабинет). Урок 23: считать по обычной ставке, факт — из
  `deductServices`/`fee`.

## 7. Лимиты запросов

- Шлюз: по IP и по ключу, «N/s/ip», «N/s/apikey»; превышение — **HTTP 429**, «IP или apiKey будет заблокирован»
  (китайская версия: 429 — предупреждение перед баном) (#documentationlimitRules). Есть ещё WAF-лимит без описания.
- Retry-After, веса, время бана — **нет в документации** → пауза `venue.rate_limit_pause_ms`, при 429 ничего не слать.
- По эндпоинтам: публичный рынок 10/с/IP; `/v4/balances` 10/с/ключ; `POST /v4/order` 20/с; `/v4/batch-order` 30/с;
  `/v4/open-order`, `/v4/history-order` 10/с; `PUT /v4/order/{id}` 50/с; **`/v4/withdraw` (и вывод, и статус) 1/с**;
  `/v4/ws-token` 1 раз в 10 с. Для `/v4/order/{id}` (GET/DELETE), `/v4/trade`, `/v4/deposit/*`, `/v4/balance` лимит не указан.
- Коды риск-контроля `GATEWAY_0001…0004` — «Trigger risk control», без описания.

## 8. Приватный WS (поток аккаунта)

- Есть. Токен: `POST /v4/ws-token` (подписанный) → `accessToken` (= listenKey), живёт 2 дня, повторный вызов продлевает
  (#websocket_privategetToken). Подписка на `wss://stream.xt.com/private` с полем `listenKey` (#websocket_privaterequestFormat);
  ответ `code` 401 «token expire» / 2 «listenKey invalid» → новый токен.
- Темы: `balance` (`c` монета, `b` «all spot balance», `f` frozen, `z` SPOT/LEVER), `order` (`st`, `eq`, `lq`, `ap`, `f` —
  комиссия без монеты), `trade` (`tm` 1 taker / 2 maker). Доступно = `b − f` — так не написано, проверить на записи.

## 9. Счета (панель «Балансы», урок 24)

| Счёт | Эндпоинт | Источник |
| --- | --- | --- |
| Спот | `GET /v4/balances` (`currencies=`), одна монета `GET /v4/balance?currency=` (решение 113) | `_balance/balancesGet.md`, `balanceGet.md` |
| Маржа | `GET /v4/lever/balance?symbol=` (по паре) | `_margin_balance/balance.md` |
| Фьючерсы USDT-M | `GET /future/user/v1/balance/list` на `fapi.xt.com`, своя подпись | `_futures_user/getBalances.md` |
| Фьючерсы Coin-M | то же на `dapi.xt.com` | `_futures_documentation/restApi.md` |
| Финансы/Earn (FINANCE) | **нет в документации** (есть только как `bizType` перевода) | `publicModule.md` |

Фьючерсный счёт может быть не открыт (`/future/user/v1/account/open`) — ответ «не открыт» = пустой счёт (урок 27);
точный код — на живой записи.

## 10. Права ключа

- Права (из создания ключа в «User center», `_user_center_apikey/createAccountApiKey.md`): QUERY, TRADE («spot and margin
  trade»), WITHDRAW, LEVER_LOAN, FUTURE, TRANSFER. Боту нужны QUERY + TRADE + WITHDRAW (+ TRANSFER, если депозит не на SPOT);
  без FUTURE и LEVER_LOAN. TRADE включает и маржу — отдельно спот не выбрать.
- Проверка ключа: `GET /v4/user/account/api-key` на `https://api.xt.com` (#user_center_apikeyqueryUserApiKey) → `roleScopes`
  (в примере только «QUERY_TRADE»/«QUERY_NO_TRADE» — о WITHDRAW не видно), `bindIps`, `isLock`. Раздел — для
  мастер/субаккаунтов; доступен ли обычному ключу — **не ясно**. **Ответ содержит `secretKey`** — не писать в журнал и в
  сырой журнал (урок 18).
- Иначе — косвенно: `AUTH_101/102` ключ, `AUTH_103` подпись, `AUTH_104` IP не привязан, `AUTH_106` нет права.

## 11. Уроки чек-листа — что важно у XT

| № | Как у XT |
| --- | --- |
| 1–3 | Документация — репозиторий `xt-com/xt4-api`; противоречия: recvWindow, `type`/`clientOrderId` вывода числом. Живых записей нет — снять до кода |
| 4 | `order_rules::ready`: PRICE/QUANTITY/QUOTE_QTY (null = без предела, шаг от min), `openapiEnabled`, `tradingEnabled`, `state`; PROTECTION_LIMIT — по цене последней сделки |
| 5, 30 | IOC есть; итоговое состояние IOC и реальное принятие — живым ордером |
| 6 | `clientOrderId` 4–32 `[a-zA-Z0-9_]`, новый на попытку |
| 7 | POST отдаёт только id → GET ордера; `deductServices` при списании в XT; лимит GET не указан |
| 8 | `contract` в справочнике сетей — есть; название сети BSC — из живого ответа |
| 9 | Справочник сетей только целиком; пары — с `version` (без изменений — пустой ответ) |
| 10 | Депозит — счёт зачисления не описан; перевод `/v4/balance/transfer` есть; **комиссия депозита `depositFeeRate`** |
| 11, 23 | Ставки по ключу нет — только ставка пары; скидка XT — только по факту |
| 12 | Снимки 1000 мс (медленно) → изменения 100 мс + REST-снимок (10/с/IP); лимиты WS не описаны; ping текстом < 60 с; permessage-deflate обязателен |
| 13–14 | Как у MEXC/Gate (без копий уровней; пересподписка не трогает фоновые круги) |
| 15 | 429 → бан; Retry-After нет — пауза настройкой |
| 16 | Приватный WS `balance`/`order` с listenKey (2 дня) |
| 21 | Минимума депозита в API нет → `venue.deposit_min_absent_ok` |
| 22 | PRE_SUCCESS нет, но есть T+1 (`WITHDRAW_009`) без способа узнать заранее |
| 24 | Спот, маржа, фьючерсы есть; Earn — нет |
| 25 | Адрес депозита с кодом «генерируется» (`DEPOSIT_007`) |
| 27 | Фьючерсы «не открыт» — пустой счёт |
| 28 | gzip REST и IPv4/IPv6 — нет в документации; WS — deflate |
| 29 | Права — `roleScopes` через user-center (доступность и полнота под вопросом); ответ содержит секрет |

## 12. Вердикт

**ПОДКЛЮЧАТЬ — после ответов на вопросы и живой записи ответов.** Спот есть, сеть BSC ожидается (название — из
справочника), адрес контракта в справочнике сетей есть, вывод/депозит через API с поиском по своему id, IOC, приватный WS.

**Сложность: средняя.** Рынок и правила пар — Binance-подобные (filters, timeInForce, `lastUpdateId`/`fi`/`i` — как у MEXC
и Binance; разбор стакана изменениями — как Gate `order_book_update` / Binance diff), но подпись своя
(`validate-*` + `#METHOD#path#query#body`), ответ `{rc, mc, result}`, отдельная подпись фьючерсов.

**Главные риски**
1. Стакан: снимок только раз в 1 с; живой стакан требует diff-потока + REST-снимков с лимитом 10/с/IP — медленный старт и
   ресинки на сотнях пар; лимиты WS не описаны (замер нагрузки, решение 109).
2. Вывод: вероятный белый список адресов (`WITHDRAW_004`), удержание T+1 (`WITHDRAW_009`) без признака в API, статус
   REVIEW (ручная проверка), лимит запросов статуса 1/с.
3. Затраты: нет ставки комиссии по ключу (только ставка пары); есть комиссия депозита `depositFeeRate` неясной семантики;
   скидка XT не видна до ордера.
4. Пары: `openapiEnabled = false`, `SYMBOL_011` (высокий риск — нужна заявка в поддержку) — часть монет может быть закрыта
   для API.
5. Документация не обновлялась с января 2025 и местами противоречива; живых ответов в этой проверке нет.

## Вопросы Тиграну (до кода)

1. Белый список адресов вывода у XT — включён ли/обязателен в кабинете? (код `WITHDRAW_004`)
2. `depositFeeRate` у сети BSC — что значит число (доля/процент) и учитывать ли как затрату? Нужна живая запись справочника.
3. Ставка комиссии: брать `takerFeeRate` пары из справочника (эндпоинта по ключу нет)? Включена ли оплата комиссии в XT?
4. Удержание T+1: как поступать, если вывод купленной монеты отказан `WITHDRAW_009`?
5. recvWindow — какое значение (документация: 5000 рекомендовано, пример 60000)?
6. Минимум депозита в API нет — разрешить `venue.deposit_min_absent_ok` для XT?
7. Нужен ключ XT для живой записи: справочник сетей (название BSC, `contract`), депозит (счёт зачисления), IOC-ордер
   (итоговое состояние), `/v4/user/account/api-key` (доступен ли обычному ключу).
