# Poloniex — анализ подключения (без кода), 05.11.2026

Источники — только официальные:
- Документация спот API: https://api-docs.poloniex.com/spot/ (разделы ниже по ссылкам).
- Официальный SDK: https://github.com/poloniex/polo-sdk-python (последний коммит c2f7c62 от 2025-02-14) — подпись,
  тексты docstring.
- Живые ответы API **не записаны**: `api.poloniex.com` из этого окружения закрыт прокси (connect_rejected). Всё, что
  помечено «проверить на живом ответе», — до кода записать в `tests/data/` (урок 3).

Сокращения ссылок: `D/` = `https://api-docs.poloniex.com/spot/`.

## 1. Спот API, адреса, подпись

- Спот есть. REST: `https://api.poloniex.com/` (D/api/). WebSocket: публичный `wss://ws.poloniex.com/ws/public`,
  приватный `wss://ws.poloniex.com/ws/private` (D/websocket/). Публичные каналы только на публичном адресе, приватные —
  только на приватном.
- Подпись REST (D/api/ «Authentication»): заголовки `key`, `signTimestamp` (мс), `signature`; необязательные
  `signatureMethod` (HmacSHA256 или Ed25519), `signatureVersion`, `recvWindow`.
  Строка: `METHOD\n` + `путь\n` + параметры. GET — все параметры + `signTimestamp`, сортировка по ASCII, `&`, значения
  URL-кодированы. POST/DELETE с телом — `requestBody=<JSON>&signTimestamp=<ts>`. HMAC-SHA256 секретом → Base64.
  Вариант Ed25519 — публичный ключ регистрируется при создании ключа.
  Сверено с SDK: `polosdk/spot/rest/request.py::_get_sig_header`.
- **Passphrase нет.**
- Время: метка живёт 1 минуту; 400 — расхождение `signTimestamp` > 1 с с часами биржи (так на странице), 408 — вышли за
  `recvWindow`. Значение по умолчанию и максимум `recvWindow` — нет в документации. Часы: `GET /timestamp` (`serverTime`).
- Противоречие в документации: `signatureVersion` в одном месте `1`, в примере curl `2` (поле необязательное — не
  слать; проверить на живом запросе, урок 2).
- Подпись приватного WS (D/websocket/authentication): сообщение `{"event":"subscribe","channel":["auth"],"params":{key,
  signTimestamp, signatureMethod?, signatureVersion?, signature}}`, строка `GET\n/ws\nsignTimestamp=<ts>`, HMAC-SHA256 →
  Base64. Ответ `{"channel":"auth","data":{"success":true|false,...}}`.
- Стиль API — как HTX/Huobi (method\npath\nsorted params, HmacSHA256+Base64, signatureMethod/Version), символы
  `BTC_USDT` как у Gate. Не Binance-подобный.

## 2. Справочник монет и сетей

Два справочника (D/api/public/reference-data):
- **v1** `GET /currencies[/{currency}]?includeMultiChainCurrencies=true` — строка на каждую сеть с отдельным именем
  монеты (пример из документации: `USDT`, `USDTETH`, `USDTTRON`). Поля: `blockchain`, `withdrawalFee`, `minConf`,
  `walletState`, `walletDepositState`, `walletWithdrawalState` (ENABLED/DISABLED), `delisted`, `tradingState`,
  `parentChain`, `childChains`, `isMultiChain`. **Адреса контракта, минимума вывода и точности в v1 нет.**
- **v2** `GET /v2/currencies` и `GET /v2/currencies/{currency}` (одна монета — урок 9 / решение 113 закрываются):
  верх: `coin`, `delisted`, `tradeEnable`, `networkList[]`; в сети: `coin` (имя монеты в этой сети, пример `TRXETH`),
  `blockchain`, `currencyType` (`address` / `address-payment-id`), `depositEnable`, `withdrawalEnable`,
  `withdrawFee`, `withdrawMin` («≤ 0 — нет минимума»), `decimals` (строка; «Currency decimals» — точность вывода или
  десятичные токена — не сказано), `minConfirm`, `depositAddress` («if available»), **`contractAddress`**
  (`null` у нативных монет).
- **Сеть BSC:** в документации и SDK название сети BSC **не встречается** (примеры — только BTC, ETH, TRX). Значение
  `blockchain` для BSC («BSC», «BEP20»…) и имя монеты в сети (например `USDTBSC`?) — **проверить на живом ответе**
  `/v2/currencies`; в настройку `venue.bsc_network` (урок 8). Не гадать.
- Адрес контракта — поле `contractAddress` в v2 → сопоставление по адресу возможно (урок 8).
- Минимум депозита — **нет в документации** (ни в справочниках, ни в адресе депозита) → `venue.deposit_min_absent_ok`
  — решение Тиграна (урок 21).
- Memo/tag: `currencyType = address-payment-id`; при выводе — `addressTag` (v2) / `paymentId` (v1). Для BEP20 memo
  не ожидается — проверять по `currencyType`, при `address-payment-id` — отказ (как у Gate/KuCoin).
- Размер справочника и gzip — нет в документации; измерить на живом ответе (уроки 9, 28).

## 3. Вывод и депозит

**Вывод** (D/api/private/wallet «Withdraw Currency»):
- `POST /v2/wallets/withdraw`: `coin`, `network`, `amount`, `address`, `addressTag?`, `allowBorrow` (слать `false`),
  `withdrawOrderId?` (до 32 символов; «duplicate values will be treated as idempotent submissions»), `withdrawalType`
  (0 = в сеть). Ответ — `withdrawalRequestsId`. Старый `POST /wallets/withdraw` — по имени мультисетевой монеты.
- «Immediately places a withdrawal … with no email confirmation», «withdrawal privilege must be enabled for your API key».
- Белый список адресов вывода — **нет в документации** (API). Есть ли он в кабинете — спросить Тиграна / проверить.
- Противоречие: `network` в документации «optional (required for on-chain)», в SDK — required. Слать всегда.
- Статусы и поиск: отдельного запроса вывода по `withdrawalRequestsId` или `withdrawOrderId` **нет**. Только
  `GET /wallets/activity?start&end&activityType=withdrawals` (start/end — Unix-время в секундах по SDK, обязательны) →
  искать свою запись в списке по `withdrawalRequestsId` / `withdrawOrderId`. Окно поиска и частота — настройки (урок 7).
- Статусы вывода — противоречие: документация `PROCESSING, AWAITING APPROVAL, COMPLETED, COMPLETE ERROR`, SDK —
  `PENDING, AWAITING APPROVAL, COMPLETE, COMPLETE ERROR` («and optionally the transaction ID» в том же поле). Проверить
  на живом выводе (урок 2). `AWAITING APPROVAL` — ручная проверка биржей (задержка, риск движения цены).
- Поля `withdrawOrderId`, `withdrawalType`, `toAccount` действуют с 15.07.2026 (changelog 2026-07-13).

**Депозит**:
- Адрес: `GET /wallets/addresses?currency=` (объект «монета → адрес»), создать — `POST /wallets/address {currency}`.
  Параметра сети нет — для BSC, видимо, нужно имя мультисетевой монеты (как `USDTTRON`) — **проверить**. SDK упоминает
  `/v2/wallets/addresses` и `/v2/wallets/address`, но в документации их **нет** — не использовать без описания.
  Есть ещё `depositAddress` в `/v2/currencies` → `networkList` — что это за адрес (общий или личный), не сказано
  (публичный запрос — скорее общий адрес для memo-монет). Вопрос Тиграну/проверка.
- История: тот же `/wallets/activity?activityType=deposits` — поля `depositNumber`, `txid`, `amount`, `confirmations`,
  `status`, `depositType` (0 = из сети). Поиска по хешу на стороне биржи нет — фильтр по `txid` у себя.
- Статусы депозита: документация `PENDING, COMPLETED`, SDK `PENDING, COMPLETE` — проверить. Статуса «предварительно
  зачислено» (как MEXC PRE_SUCCESS) в документации **нет** → `withdraw_hold` неприменим по документации (урок 22).
- Счёт зачисления — прямо не написано; у спот API один счёт: «accountType … SPOT is the default and only supported
  value», funding/main нет → внутренний перевод не нужен (`deposit_to_trading` ничего не делает), подтвердить на живом
  депозите.

## 4. Ордера

(D/api/private/order)
- `POST /orders`: `symbol` (`BTC_USDT`), `side`, `type` = `LIMIT` | `MARKET` | `LIMIT_MAKER`, **`timeInForce` =
  `GTC` | `IOC` | `FOK`** (стартовое правило «не исполнился — отменяем» закрывается IOC, урок 5), `price`, `quantity`,
  `clientOrderId`, `allowBorrow` (false), `stpMode` (по умолчанию EXPIRE_TAKER), `slippageTolerance`.
  Ответ: `id`, `clientOrderId`. Внимание: по умолчанию `type = MARKET` — всегда слать явно.
- Свой id: «Max 64 characters … [A-Za-z0-9_-]». **Противоречие**: ошибка 21315 «Client orderId exceeded max length of
  17 digits». Новый id на каждую попытку (урок 6), длину — проверить живым ордером, в настройку.
- Итог: `GET /orders/{id}` и `GET /orders/cid:{clientOrderId}` — `state` (NEW, PARTIALLY_FILLED, FILLED,
  PENDING_CANCEL, PARTIALLY_CANCELED, CANCELED, FAILED), `filledQuantity`, `filledAmount`, `avgPrice`, `cancelReason`.
  **Комиссии в ордере нет** — только в сделках: `GET /orders/{id}/trades` (`feeAmount`, `feeCurrency`, `matchRole`,
  `price`, `quantity`) или в приватном WS `orders` (`tradeFee`, `feeCurrency`). Итог = ордер + его сделки (урок 7).
- Правила пары: `GET /markets[/{symbol}]` → `state` (NORMAL / PAUSE / POST_ONLY), `symbolTradeLimit`: `priceScale`,
  `quantityScale`, `amountScale` (число знаков, шаг = 10^−scale), `minQuantity`, `minAmount`, `maxQuantity`,
  `maxAmount` (0 — без проверки), `highestBid` / `lowestAsk` (границы цены). Ещё: ценовая защита 20 % от лучшей цены
  (ошибка 21356, D/trading-rules), «Amount must be greater than 1 USDT» (21350). Всё — в `order_rules::ready` (урок 4).
- Пробного ордера **нет в документации** → `venue.order_test_first` = да для Poloniex — ошибка настройки (как у Gate).
- Есть ордера через приватный WS (`createOrder`, D/websocket/trade-request) и kill switch `POST /orders/killSwitch` —
  в первой версии не нужны.

## 5. Стакан

(D/websocket/market-data, D/api/public/market-data)
- `book` — снимок 5/10/20 уровней (`depth`), «updated every 100ms», поля `symbol, asks, bids, id, ts, createTime`.
  Приходит снимками — отдавать ядру только при изменении (решение 109, урок 13).
- `book_lv2` — снимок 20 уровней, затем изменения (`action` snapshot/update); `id`/`lastId`: «If id of the last message
  does not match lastId of the current message» → переподписка (урок 12, без бесконечного цикла ресинков).
- Подписка: `{"event":"subscribe","channel":["book"],"symbols":[...]}`. Сколько символов в подписке / подписок на
  соединение — **нет в документации** (измерить, в `venue.ws_symbols_per_conn`). Лимиты: 2000 соединений с IP на
  публичный и приватный адрес, 500 запросов/с на соединение.
- Пинг: `{"event":"ping"}` → `{"event":"pong"}`; «expects a message or a ping every 30 seconds» — иначе обрыв без
  предупреждения. Сервер сам пинг не шлёт.
- REST снимок: `GET /markets/{symbol}/orderBook?limit=` (5, 10, 20, 50, 100, 150), `scale` не слать (без агрегации).

## 6. Комиссии

- `GET /feeinfo` (D/api/private/account): `makerRate`, `takerRate`, `volume30D`, `trxDiscount` (Boolean),
  `specialFeeRates[]` (`symbol`, `makerRate`, `takerRate`) — ставка по паре = special, иначе общая. Один запрос на все
  пары (урок 11, фоновый круг с `venue.fee_retry_s`, урок 26).
- Скидка своей монетой: признак `trxDiscount` (скидка за TRX). Как включается, размер скидки и что при нехватке TRX —
  **нет в документации** API. По уроку 23: считать по обычной ставке, факт и монета комиссии — из сделок
  (`feeCurrency`). В какой монете берётся комиссия покупки (base или quote) — нет в документации, проверить живым ордером.

## 7. Лимиты запросов

(D/api/ «Rate Limits», changelog 2025-03-13: лимиты зависят от уровня VIP)
- Приватные — по UID, VIP0: ордер/отмена/детали ордера/балансы/feeinfo/адреса — 50; `POST /wallets/withdraw` — 10;
  `/wallets/activity` — 10. Единица времени в таблице приватных не подписана (у публичных — «в секунду»).
- Публичные — по IP: одиночные (`/markets/{symbol}…`) 200/с, списки (`/markets`) 30/с; у `/currencies`,
  `/v2/currencies`, `orderBook` значение в таблице пустое.
- Ответ при превышении (429/418, Retry-After, бан) — **нет в документации** → пауза `venue.rate_limit_pause_ms`
  (как у Gate), коды записать на живом ответе (урок 15). Весов/корзин нет.
- Коды ошибок (D/error-code): 21709 нехватка баланса, 21312 дубль clientOrderId, 21322/21330 ниже минимума,
  21335–21337 масштаб, 21340/21341 обслуживание / только post-only, 21351–21354 заморозки, 24201 «Service busy».
  Кодов ключа/подписи/IP/прав в таблице **нет** — записать живые ответы.

## 8. Приватный поток аккаунта

- Есть: `balances` на `wss://ws.poloniex.com/ws/private` после `auth` (D/websocket/balance): `currency`,
  `available`, `hold`, `accountType` (SPOT), `eventType` (place_order, canceled_order, match_order, transfer_in,
  transfer_out, deposit, withdraw), `version`, `changeTime`. Токена/listenKey нет — подпись в сообщении `auth`.
- `orders` (D/websocket/order) — исполнение с `tradeFee`/`feeCurrency` → сырой журнал / проверка факта (урок 16).
- При сбое ключа — «you need to re-authenticate».

## 9. Счета (панель «Балансы», урок 24)

- Спот: `GET /accounts/balances?accountType=SPOT` (`available`, `hold`), также `GET /accounts` (`accountState`
  NORMAL/LOCKED). «Currently only SPOT is supported». Баланс одной монеты отдельным запросом — нет в документации.
- Фонд/funding — **нет** такого счёта.
- Маржа (кросс): `GET /margin/accountMargin` (D/api/private/margin) — только чтение.
- Фьючерсы: отдельный API v3 (`GET /v3/account/balance`, https://api-docs.poloniex.com/v3/futures/api/) и
  **отдельные ключи** («Futures API Keys», D/../faq) — без фьючерсного ключа счёт показывать с error «нет ключа».
  Фьючерсный ключ боту не нужен (только спот).
- Earn/стейкинг — эндпоинта в спот API **нет**.
- Перевод между счетами `POST /accounts/transfer` — не нужен.

## 10. Права ключа

- В документации спот API права ключа не описаны, кроме «withdrawal privilege must be enabled for your API key».
  Ключи создаются на https://poloniex.com/profile/settings/spot-api-key (D/../faq).
- Проверить права ключа через API — **нет эндпоинта** (нет аналога Gate `/account/main_keys`, KuCoin `/api/v1/user/api-key`).
  «Проверить подключение» может проверить только подпись и доступ на чтение (`GET /accounts`, `accountState`); право
  торговли и вывода — не проверяемо без действия (урок 29). Вопрос Тиграну: допустимо ли так, или проверять торговлю
  пробным действием (например, отмена несуществующего ордера → 21301 vs отказ в правах — код отказа неизвестен).
- IP-белый список ключа — нет в документации API.

## 11. Уроки чек-листа — что важно для Poloniex

| Урок | Poloniex |
| --- | --- |
| 1–3 | Документация тонкая: нет кодов 429/подписи/прав, нет имени сети BSC. Сначала записать живые ответы (`/v2/currencies`, `/markets`, `/feeinfo`, WS `book`, ошибки) — прокси здесь закрыт. |
| 2, 30 | Противоречия: длина clientOrderId (64 vs 17), статусы депозита/вывода (COMPLETED vs COMPLETE, PROCESSING vs PENDING), `signatureVersion` 1/2, `network` в выводе. Проверять живыми ответами. IOC в справочнике пар не перечисляется вовсе (нет `orderTypes`) — подтвердить живым ордером. |
| 4 | `order_rules::ready`: scale → шаги, min/maxQuantity, min/maxAmount, highestBid/lowestAsk, `state` = NORMAL (PAUSE/POST_ONLY — нельзя), ≥ 1 USDT, 20 % защита. Пробного ордера нет. |
| 5 | LIMIT + IOC — есть. |
| 6 | clientOrderId новый на попытку; `withdrawOrderId` — идемпотентный ключ вывода, тоже новый на попытку. |
| 7 | Комиссия — только из сделок ордера / WS `orders`; окно `/wallets/activity` — настройки. |
| 8 | Сопоставление по `contractAddress` из v2; имя сети BSC — из живого справочника в `venue.bsc_network`. Связь v2 (`coin`+`blockchain`) ↔ имя мультисетевой монеты v1 для адреса депозита — проверить. |
| 9, 28 | Одна монета — `/v2/currencies/{currency}`; разбор вне потоков tokio; gzip/IPv4 — нет в документации, измерить. |
| 10 | Статус ввода/вывода, комиссия, минимум вывода (≤ 0 — нет), подтверждения есть; точность (`decimals`) — смысл неясен. Зачисление на спот (единственный счёт). |
| 12–14 | `book` снимками каждые 100 мс → отсев неизменных (решение 109); либо `book_lv2` с id/lastId. Лимит символов на соединение — измерить. Ping ≤ 30 с — настройка. |
| 15 | Лимиты по UID/IP без весов; ответ при превышении не описан → пауза настройкой. `wallets/activity` и вывод — 10 (ед./с не подписана) — не опрашивать часто. |
| 16 | Приватный WS `balances`/`orders` есть, подпись сообщением `auth`. |
| 19, 29 | Права проверить нельзя; белый список адресов — нет в API. |
| 21 | Минимума депозита в API нет → `venue.deposit_min_absent_ok`. |
| 22 | Статуса «предварительно зачислено» нет в документации → `withdraw_hold` неприменим (подтвердить). |
| 23 | Скидка TRX (`trxDiscount`) — расчёт по полной ставке. |
| 24 | Спот + маржа (+ фьючерсы отдельным ключом); funding/earn нет. |
| 25 | Адрес депозита — `/wallets/addresses?currency=` с кэшем, в предпроверке. |
| 27 | Аналог «счёт не открыт» — нет в документации. |

Прочее (не из документации, общеизвестно): Poloniex связан с HTX (в API — переводы на HTX); в ноябре 2023 биржу
взламывали (вывод горячих кошельков). Это риск контрагента, а не API — решать Тиграну. Торговля для клиентов из США
закрыта (код 21353).

## 12. Вердикт

**ПОДКЛЮЧАТЬ — условно, после записи живых ответов.** Есть спот, IOC у лимитного ордера, вывод через API без
подтверждения по почте с идемпотентным `withdrawOrderId`, справочник v2 с `contractAddress`, статусами ввода/вывода,
комиссией и минимумом вывода, приватный WS балансов и ордеров, ставки комиссии по паре.

Блокеры до кода (вопросы Тиграну / живая запись):
1. Есть ли BSC (BEP20) в `/v2/currencies` и как называется `blockchain`; как получить адрес депозита BSC (имя
   мультисетевой монеты v1 или `depositAddress` v2).
2. Реальные статусы депозита и вывода (COMPLETED/COMPLETE, PROCESSING/PENDING).
3. Длина `clientOrderId` (64 или 17).
4. Ответ при превышении лимита и коды ошибок подписи/прав.

**Сложность: средняя.** Подпись и формат — как у HTX (новый для проекта стиль, но простой, без passphrase); один
спот-счёт без внутренних переводов (проще KuCoin); но нет поиска вывода/депозита по id/хешу (только список по окну
времени), нет проверки прав ключа, нет пробного ордера (как Gate), комиссия только в сделках, документация тонкая.
Ближе всего к Gate (символ `BASE_QUOTE`, нет test order, снимки стакана N уровней + diff с номерами).
