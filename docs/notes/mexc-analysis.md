# MEXC — системный анализ перед интеграцией

Claude, 02.11.2026. Основа — официальная документация MEXC Spot V3 и официальные `.proto`. Каждый пункт сверен с уроками BingX
(решения 94–105 и разборы 01–02.11.2026). Кода ещё нет. Решения по открытым пунктам принимает Тигран.

## Источники и насколько им верить

| Метка | Источник | Насколько свежий |
|---|---|---|
| [L] | Живые страницы `https://www.mexc.com/api-docs/spot-v3/...` | Текущие. Прочитаны через пересказчик, поэтому **перед кодом каждую цифру сверить на живом ответе API** |
| [G] | Официальный репозиторий `mexcdevelop/apidocs`, ветка `spot_v3_en` | Changelog обрывается на 2025-02; там, где расходится с [L], верим [L] |
| [P] | Официальные `.proto`: `github.com/mexcdevelop/websocket-proto` | Свежие (2026-09-09), дословно |
| — | Postman `mexcdevelop/mexc-api-postman` | 2023 год, устарел, не используем |

Из рабочей среды Claude API MEXC недоступен: сеть закрыта, прокси отвечает 403. **Живые ответы записываем на сервере бота** (см. «План»).

## 1. Уроки BingX против MEXC

| # | Что случилось с BingX | Как у MEXC по документации | Что делаем |
|---|---|---|---|
| 1 | Депозит пришёл на **фондовый счёт**, бот видел только спот (решение 102) | Фондового счёта в доках **нет**. Универсальный перевод знает только SPOT / FUTURES / OTC. В потоке спот-аккаунта есть тип изменения `DEPOSIT` — косвенный признак, что депозит идёт на спот | `deposit_to_trading` для MEXC = «не нужно». **Проверить на первом депозите**: если окажется не на споте — включить перевод (эндпоинт есть: `POST /api/v3/capital/transfer`, вес 50) |
| 2 | Сумма вывода с «хвостом» (14.42555999…) → отказ «Precision: 8» (решение 104) | Числа знаков в сумме вывода в доках **нет**. Есть ошибка 10096 «amount decimal places is too long» и поле сети `withdrawIntegerMultiple` («Withdraw amount increment», в примере `"18"` или `null`) — **смысл неясен** | Округление вниз уже делается в ядре (`floor_precision`). Как перевести `withdrawIntegerMultiple` в точность — **только по живому ответу** getall и по пробному выводу |
| 3 | После отклонённого вывода монеты оказались на фандинге (ATOM) | Фандинга нет | Риск ниже. Вывод — сразу со спота (у MEXC поля «с какого счёта» в выводе нет) |
| 4 | Комиссия покупки пришла числом с плавающей точкой (0.014440000000000001) | `tradeFee` отдаёт **числа**, не строки (`0E-18`, `0.000500000000000000`); myTrades: `commission` / `commissionAsset` | Числа читаем через текст JSON (как в `bingx/num.rs`), без f64. Количество после комиссии — тем же округлением вниз |
| 5 | Флаги «торгуется через API» у пары (apiStateBuy/Sell) | `isSpotTradingAllowed` в exchangeInfo, **плюс** списки `GET /api/v3/defaultSymbols` (платформа) и `GET /api/v3/selfSymbols` (наш ключ). Пары ключу задаются в кабинете: «My API Key — Trading Pairs». Ошибки 30020, 70011 — «нет прав на пару» | Монета торгуется, только если пара есть в **selfSymbols** нашего ключа и `isSpotTradingAllowed`. Иначе — точная причина в `SpotPairs` («пара не разрешена ключу — включите в кабинете MEXC») |
| 6 | Статусы депозита в официальных источниках расходились | 12 статусов: 1 SMALL, 2 TIME_DELAY, 3 LARGE_DELAY, 4 PENDING, **5 SUCCESS**, 6 AUDITING, 7 REJECTED, 8 REFUND, **9 PRE_SUCCESS**, 10 INVALID, 11 RESTRICTED, **12 COMPLETED**. Какой из 5 / 9 / 12 значит «можно продавать» — не сказано | «Зачислено» = **5 или 12** и `confirmTimes ≥ minConfirm`. 9 (PRE_SUCCESS) — не зачислено. **Проверить на первом депозите** |
| 7 | Поток баланса BingX так и не проверен вживую | listenKey (60 мин, продлевать раз в 30), до 60 ключей на UID, 5 соединений на ключ, 24 ч жизни. Канал `spot@private.account.v3.api.pb` в **protobuf**, тип `DEPOSIT` / `WITHDRAW`. **Хеша транзакции в событии нет** | Как у BingX: событие только будит опрос, зачисление закрывает статус депозита по REST (`transfer.credit_by_event` [mexc] не включать до проверки) |
| 8 | Исход ордера при таймауте неизвестен | [G]: на 5XX «the execution status is UNKNOWN and could have been a success» | Тот же OutcomeUnknown (решение 64). У MEXC **есть поиск по своему ID** (`origClientOrderId` в запросе и отмене) — можно найти ордер, а не только остановиться |
| 9 | KyberSwap 429 и очереди | Лимиты: IP — 300 веса за 10 с, UID — 500 веса за 10 с, ордера + отмены — **12 запросов/с** на UID. 429 и 418 с заголовком `Retry-After`; продолжать после 429 — бан IP от 2 минут до 3 суток | Свой темп по весам (настройки), уважать `Retry-After`, 429 — пауза площадки по правилу шлюза. Не дёргать exchangeInfo (вес 25) и depth (вес 3) часто |
| 10 | Сеть BSC называется по-своему (BingX: BEP20) | Отображаемое имя `"BEP20(BSC)"`, а код `netWork` для нового вывода **в доках не указан** (у других сетей: `ARB`, `TRX`) | `venue.bsc_network` [mexc] — значение из **живого** getall. Сопоставление сети — по полю `netWork` (новый вывод), отображаемое `network` устаревает |
| 11 | Адрес контракта (`contractAddress`) не всегда приходил | В getall поле сети `contract`. В exchangeInfo тоже есть `contractAddress`, но в примере `"xtokens"` | Адрес — **только** из getall (`contract` сети BSC). Нет адреса — монета не проходит первичный фильтр (как сейчас) |
| 12 | asks в стакане BingX приходили не в том порядке | Порядок в protobuf не оговорён | Сортируем сами, как у BingX |
| 13 | Права ключа и привязка IP («Проверить подключение») | `GET /api/v3/apiKeyInfo`: `permissions`, `ipWhiteList`, `status`, `remainingValidity`. **Ключ без IP живёт 90 дней.** Права: SPOT_ACCOUNT_READ, SPOT_DEAL_WRITE, SPOT_WITHDRAW_READ/WRITE, SPOT_TRANSFER_* | «Проверить подключение»: нужные права, IP сервера в белом списке, срок ключа. Остаток срока < N дней — уведомление (настройка) |
| 14 | Вывод BingX требует белого списка адресов | Обязателен ли белый список для вывода через API — **не сказано**. Есть ошибка 10212 «not on the approved commonly used address list» и адресная книга `GET /api/v3/capital/withdraw/address` | Заранее добавить адрес кошелька бота в адресную книгу MEXC (сеть BSC). «Проверить подключение» показывает, есть ли он там |
| 15 | Защита вывода | Ошибка 157005 «Withdrawal is currently locked by withdrawal protection», 10265 — риск-контроль | Это WithdrawRejected с точной причиной, монеты на месте → «застрявшая монета» |

## 2. Что у MEXC иначе, чем у BingX (новое для кода)

1. **WebSocket в protobuf.** С 04.08.2025 старый JSON-адрес отключён. Новый: `wss://wbs-api.mexc.com/ws`, двоичные кадры `PushDataV3ApiWrapper` (официальные `.proto`). Нужна зависимость `prost` и вендоринг `.proto` в репозиторий. Подписка и PING — JSON-текстом, данные — двоичные.
2. **30 подписок на соединение**, до 100 запросов/с на сервер; предела соединений с одного IP в доках нет. При ~200 монетах — 7+ соединений; число монет на соединение — настройка.
3. **Стакан.** Два пути:
   - `spot@public.limit.depth.v3.api.pb@<SYM>@20` — частичный стакан, 5/10/20 уровней, каждый кадр — полный снимок. Просто, без пересборки. Частота не указана;
   - дифф `spot@public.aggre.depth.v3.api.pb@100ms@<SYM>` + снимок REST `depth` (limit до 5000) с проверкой версий (`fromVersion` = прошлый `toVersion` + 1, иначе пересобрать).
   Предлагаю начать с **частичного 20 уровней** (как у BingX `@depth<N>`): меньше логики и REST-веса. Дифф — если 20 уровней не хватит глубины для доли h.
4. **Шага цены и шага количества (tickSize/stepSize) нет.** Есть целые `baseAssetPrecision`, `quotePrecision`, `quoteAssetPrecision` (без объяснения), `baseSizePrecision` — это **минимальное количество**, `quoteAmountPrecision` — **минимальная сумма** ордера. Отдельного кода ошибки на неверный шаг нет (только 33333 «param is error»). **Что значат целые precision — знаки после запятой или нет — только по живому exchangeInfo и пробному ордеру (`POST /api/v3/order/test`).**
5. **IOC — это тип ордера** `type=IMMEDIATE_OR_CANCEL`, параметра timeInForce нет. Обязательные поля для IOC не описаны (по смыслу — `quantity` + `price`, проверить `order/test`). `venue.order_time_in_force` у MEXC не применяется — нужна своя настройка типа ордера или правило адаптера.
6. **Ответ на ордер без статуса и исполнения.** Итог — только запросом `GET /api/v3/order` (вес 2) или из потока `spot@private.orders.v3.api.pb`. Ключ суммы: `cumulativeQuoteQty` [L] или `cummulativeQuoteQty` [G] — по живому ответу.
7. **Ценовой фильтр `PERCENT_PRICE_BY_SIDE`.** Для LIMIT / IOC / FOK: покупка `price ≤ lastPrice × (1 + bidMultiplierUp)`, продажа `price ≥ lastPrice × (1 − askMultiplierDown)`. Наша «худшая цена» может выйти за фильтр → отказ. Проверять в адаптере до отправки.
8. **Подпись.** Строка = queryString + тело **без `&` между ними**, порядок параметров при подписи = порядок отправки, hex в нижнем регистре, заголовок `X-MEXC-APIKEY`, recvWindow до 60 000 (советуют < 5000). **Все параметры — в query string** (так проще и однозначно); `Content-Type: application/x-www-form-urlencoded` — **нельзя** (ошибка 700013).
9. **Вывод:** новый `POST /api/v3/capital/withdraw` (`coin`, `netWork`, `contractAddress`, `address`, `amount`, `withdrawOrderId`). Старый `withdraw/apply` — «offline soon». Статусы вывода: 1 APPLY … 6 WAIT_CONFIRM, **7 SUCCESS**, **8 FAILED**, **9 CANCEL**, 10 MANUAL. Хеш — `txId` или `transHash` (в примере `txId: null`, `transHash` заполнен) — брать любой непустой, проверить вживую.
10. **Ставки комиссии:** `GET /api/v3/tradeFee?symbol=` (вес 20) — по паре. Плюс `makerCommission` / `takerCommission` в exchangeInfo. Скидка при оплате комиссии в MX — **не включать** (иначе комиссия в MX, не в монете сделки, расчёт другой).
11. **Лимит 500 открытых ордеров** на аккаунт, у нас IOC — неважно.

## 3. Ядро: что решить до интеграции (касается любой второй биржи)

1. **Биржа ↔ биржа (BingX ↔ MEXC).** `core/registry.rs` создаст это направление сам. В CLAUDE.md его нет → решение Тиграна и запись в CLAUDE.md, либо переключатель «выключено».
2. **`money.max_per_coin_usd` = 45** при сделке 44 → одна сделка на монету одновременно на всех биржах.
3. **Капитал MEXC** — отдельный USDT на споте MEXC; возврат USDT («в каждой сделке» / «по порогу») и «Перегон» начнут учитывать MEXC.
4. **Возврат USDT с MEXC на кошелёк** — комиссия вывода USDT в BEP20 у MEXC (из живого getall) войдёт в расчёт направления DEX → MEXC так же, как 0.15 у BingX (техдолг ПРИОРИТЕТ 1).
5. **Правила шлюза для mexc** — стартовое правило Тиграна действует и здесь; коды MEXC переводятся в единые.

## 4. План

**Фаза 1 — только чтение (без денег).**
- Адаптер `src/venues/mexc/`: подпись, справочник пар (exchangeInfo + selfSymbols + defaultSymbols), монеты и сети (getall), ставки комиссии, стакан WS protobuf (частичный 20), балансы, «Проверить подключение» (apiKeyInfo).
- Все ответы и кадры — в сырой журнал (`raw.*`), как у BingX.
- Секреты `mexc.api_key`, `mexc.api_secret`; настройки `venue.*` [mexc].
- **Торговля по MEXC выключена** (покупка и продажа запрещены флагом адаптера, пока нет фазы 2).
- После выкладки на сервер — записи живых ответов, по ним закрываем все пункты «проверить вживую».

**Фаза 2 — деньги, после записей.**
- Ордер IOC (с `order/test` перед первым живым), итог ордера, поиск по своему ID.
- Вывод (новый эндпоинт), статусы, адрес депозита, история депозитов.
- Поток аккаунта (protobuf), события баланса.
- Первые сделки — малым бюджетом.

**Что нужно от Тиграна для фазы 1:**
- API-ключ MEXC: права SPOT_ACCOUNT_READ, SPOT_DEAL_READ/WRITE, SPOT_WITHDRAW_READ/WRITE (для фазы 2), **привязка к IP сервера** (иначе ключ живёт 90 дней), пары ключа — включить нужные (или все).
- Ответы на 3 вопроса ядра (раздел 3).

## 5. Чего в документации нет (проверяем на живых ответах)

1. Схема ответа об ошибке (встречаются `{code, msg}` и `{code, data, msg, timestamp}`).
2. Смысл целых `*Precision`; шаг цены и количества.
3. Обязательные поля IOC.
4. Как работают defaultSymbols / selfSymbols.
5. Счёт зачисления депозита.
6. Код `netWork` для BSC.
7. Белый список адресов для вывода через API.
8. Число знаков суммы вывода, смысл `withdrawIntegerMultiple`.
9. Когда можно отменить вывод.
10. Веса ордеров, отмены, listenKey.
11. Предел WS-соединений с одного IP; формат ошибки подписки.
12. События депозита и вывода в потоке (только изменение баланса, без хеша).
13. Дата отключения `withdraw/apply`.

## 6. Противоречия в официальных источниках

1. Лимит IP: [G] «500 every 10 second» ↔ [L] «300 weight per 10 seconds».
2. Бан: [L] «2 minutes … 3 days» ↔ FAQ «resume to normal after 10 minutes».
3. Белый список IP ключа: FAQ «10 IP» ↔ `POST apiKeyInfo` «up to 20».
4. Тело запроса: «raw parameter string, not JSON» ↔ «application/json is supported».
5. Пинг: клиент шлёт `{"method":"PING"}` ↔ FAQ «Pong reply is required upon receiving the Ping … from the server».
6. Статус пары: `"1"` [L] ↔ `"ENABLED"` [G].
7. `cummulativeQuoteQty` [G] ↔ `cumulativeQuoteQty` [L]; в запросе ордера `Qty` вместо `origQty`.
8. apiKeyInfo: поле `apikey` в таблице ↔ `accessKey` в примере.
9. Тип изменения баланса `CONTRACT_TRANSFER` в примере, которого нет в перечне.

## Ссылки (официальные)

- Введение, подпись, лимиты: https://www.mexc.com/api-docs/spot-v3/introduction
- Коды ошибок: https://www.mexc.com/api-docs/spot-v3/error-code
- exchangeInfo: https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/exchange-information
- Пары API: https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/api-default-symbol, https://www.mexc.com/api-docs/spot-v3/spot-account-trade/user-api-default-symbol
- Стакан REST: https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/order-book
- WS рынка: https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/ ; protobuf: https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/protocol-buffers-integration ; локальный стакан: https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/how-to-properly-maintain-a-local-copy-of-the-order-book
- `.proto`: https://github.com/mexcdevelop/websocket-proto
- Ордер: https://www.mexc.com/api-docs/spot-v3/spot-account-trade/new-order ; тест: …/test-new-order ; запрос: …/query-order ; отмена: …/cancel-order ; сделки: …/account-trade-list ; комиссии: …/query-symbol-commission
- Аккаунт: https://www.mexc.com/api-docs/spot-v3/spot-account-trade/account-information ; ключ: …/query-api-key-info
- Кошелёк: https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/query-the-currency-information ; вывод: …/withdrawnew ; история выводов: …/withdraw-history-supporting-network ; адрес депозита: …/deposit-address-supporting-network ; история депозитов: …/deposit-historysupporting-network ; перевод: …/user-universal-transfer
- Поток аккаунта: https://www.mexc.com/api-docs/spot-v3/websocket-user-data-streams/listen-key ; баланс: …/spot-account-update
- Смена WS: https://www.mexc.com/announcements/article/mexc-v3-websocket-service-replacement-announcement-17827791522393
- Change log: https://www.mexc.com/api-docs/spot-v3/change-log
