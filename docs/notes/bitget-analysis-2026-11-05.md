# Bitget — анализ перед подключением (без кода)

Claude, 05.11.2026. Основа — официальная документация Bitget (новый адрес https://www.bitget.com/docs/ ; старый
`https://www.bitget.com/api-doc/...` теперь переадресует на обзор UTA) и официальный SDK
https://github.com/BitgetLimited/v3-bitget-api-sdk (коммит `9e6ee01` от 27.08.2026: `bitget-python-sdk-api/bitget/*`,
Postman-коллекция UTA `upex-postman-openapi/`). Чек-лист — все 30 уроков `docs/notes/cex-integration-lessons-2026-11-04.md`.

**Как читалось и чего не удалось проверить.** Из среды Claude `www.bitget.com` и `api.bitget.com` через curl закрыты
(прокси 403); страницы документации прочитаны через пересказчик — дословные цитаты в кавычках, остальное пересказ.
Живых ответов API нет ни одного. **Перед кодом каждую цифру и поле сверить на живом ответе с сервера бота** (как MEXC,
решение 133). Не удалось прочитать: страницы приватных каналов WS спота (Account / Orders — старые адреса
переадресуют на обзор UTA), страницу «Rate Limit», полный список кодов ошибок (страница `docs/api/error-code` —
«демо», два кода), разделы UTA по счетам и депозитам (страницы-каталоги с переадресацией). Это отмечено по месту.

## Главное: у Bitget два API — Classic (v2) и UTA (v3)

- **Classic** — `/api/v2/...`, отдельные счета (спот, фонд/P2P, фьючерсы, маржа). Документация:
  https://www.bitget.com/docs/classic/catalog (разделы «Spot → Market / Account / Trade», «Public → Public Config /
  Assets»).
- **UTA (Unified Trading Account)** — `/api/v3/...`, единый счёт, спот через `category` (Postman UTA:
  `POST /api/v3/trade/place-order`, `GET /api/v3/market/instruments?category=`, `GET /api/v3/account/assets`).
  Документация: https://www.bitget.com/docs/uta/catalog .
- Статус Classic («Upgrade to Unified Trading Account», https://www.bitget.com/docs/classic/uta-api-upgrade-guide):
  срока отключения v2 **нет**; «New products and capabilities are prioritized for release on UTA»; «Your existing v2
  API Key automatically gains UTA access». Работают ли v2-эндпоинты спота на счёте, переведённом в UTA, — **в
  документации не сказано.**
- В UTA-SDK и Postman UTA **нет справочника монет с сетями** (адрес контракта, комиссии вывода) — он есть только в
  Classic `GET /api/v2/spot/public/coins` (публичный, ключ не нужен).
- **Предложение:** адаптер на Classic v2 (как KuCoin Classic, решение 137); «Проверить подключение» показывает режим
  счёта; UTA — отказ с точной причиной. **Вопрос Тиграну:** счёт Bitget бота — классический? (переход в UTA —
  `POST /api/v2/spot/account/upgrade`, «Upgrade Account»; обратим ли — в документации не сказано).

## 1. Спот API, адреса, подпись

- Спот есть (Classic «Spot», UTA `category=SPOT`). Только спот — фьючерсы и маржа не трогаем.
- REST: «https://api.bitget.com» (Classic → REST API, https://www.bitget.com/docs/classic/rest-api ; SDK
  `consts.py: API_URL`).
- WebSocket Classic: публичный `wss://ws.bitget.com/v2/ws/public`, приватный `wss://ws.bitget.com/v2/ws/private`
  (https://www.bitget.com/docs/classic/websocket/intro).
- Подпись REST (Classic → REST API → Signature): prehash = «timestamp + method.toUpperCase() + requestPath + "?" +
  queryString + body» (без `?` при пустой строке запроса), HMAC SHA256 секретом, Base64. Заголовки `ACCESS-KEY`,
  `ACCESS-SIGN`, `ACCESS-TIMESTAMP` (миллисекунды), `ACCESS-PASSPHRASE`, `Content-Type: application/json` для POST,
  `locale`. Возможна подпись RSA (SHA-256 + Base64; SDK `utils.signByRSA`) — нам не нужна. Время: «within 30 seconds
  of the API server time». Сверено с SDK `utils.py: pre_hash/sign`.
- **Passphrase — да**, обязателен → в секреты (урок 18), как KuCoin/OKX.
- У ордера есть `requestTime` / `receiveWindow` (окно годности запроса) — можно не задавать.

## 2. Справочник монет и сетей

`GET /api/v2/spot/public/coins` — «Spot → Market → Get Coin Info»
(https://www.bitget.com/docs/catalog/classic-spot-market/classic-spot-market#get-coin-info). Лимит «3 times/1s (IP)».
Параметр `coin` — одна монета («If the field is left blank, all coin information will be returned») → решение 113
закрывается (`coin_networks_of`). Ключ не нужен — можно записать живой ответ с сервера бота сразу.

Поля монеты: `coinId`, `coin`, `transfer`, `chains[]`. Поля сети (`chains[]`):

| Поле | Смысл по документации | Нам |
|---|---|---|
| `chain` | «Chain name» | Имя сети BSC **в документации нет** (пример — только `"BTC"`). Из живого ответа → `venue.bsc_network` (урок 8) |
| `contractAddress` | «coin contract address» | Сопоставление по адресу (урок 8). В примере у BTC стоит адрес USDT в Ethereum — пример небрежный, заполненность для BSC проверить вживую |
| `withdrawable` / `rechargeable` | «Withdrawal supported» / «Deposit supported», строки `"true"/"false"` | Статусы ввода/вывода |
| `withdrawFee` | «Withdrawal transaction fee» | Комиссия вывода |
| `extraWithdrawFee` | «Extra charge. On chain destruction: `0.1` means `10%`» | **Доля от суммы** — входит в стоимость вывода (урок 10); при ≠0 — вопрос, как считать |
| `minWithdrawAmount` | «Minimum withdrawal amount» | Минимум вывода |
| `withdrawMinScale` | «Decimal places of withdrawal amount» | Точность суммы вывода |
| `withdrawStep` | «If the value is not 0, … withdrawal size should be multiple of the value» | Кратность суммы (вторая проверка рядом с точностью) |
| `minDepositAmount` | «Minimum deposit amount» | **Минимум депозита прямо в справочнике** — фоновый круг адресов (урок 21) не нужен. Семантика `"0"`/пусто не описана → `venue.deposit_min_absent_ok` |
| `needTag` | «Need tag» | memo/tag: при `"true"` — отказ (как Gate/KuCoin) |
| `depositConfirm` / `withdrawConfirm` | «confirmation blocks» | Подтверждения |
| `congestion` | `normal` / `congested` | Перегрузка сети — можно показывать причину |
| `browserUrl` | обозреватель | — |

Размер полного ответа — неизвестен (вживую замерить; урок 9: разбор вне рабочих потоков, кэш за `Arc`). Про gzip
(урок 28) — в документации нет; IPv6 — не сказано.

Справочник пар `GET /api/v2/spot/public/symbols` («Get Symbol Info», «20 times/1s (IP)», параметр `symbol` для одной пары).

## 3. Вывод и депозит

**Вывод** `POST /api/v2/spot/wallet/withdrawal` — «Spot → Account → Withdraw»
(https://www.bitget.com/docs/catalog/classic-spot-account/classic-spot-account#withdraw), «5 req/sec/UID».
- Параметры: `coin`, `transferType=on_chain`, `address`, `chain` (обязателен для on_chain), `size` (точность — из
  Get Coin Info), `tag`, `remark`, `clientOid` («Client Unique Customized Id»). Ответ: `orderId`, `clientOid`.
- **Белый список адресов:** в примечании к выводу — «Coin withdrawals including on-chain withdrawals and internal
  transfers (the address needs to be added in the address book on web)». Код 11000: «Withdrawal whitelist is enabled;
  your account can only withdraw to address-book addresses» (https://www.bitget.com/docs/api/error-code). → Адрес
  кошелька бота (и адреса других бирж для CEX ↔ CEX) заранее занести в адресную книгу Bitget в кабинете. Урок 19.
- IP: «you can only withdraw coins through a whitelisted IP address» (REST API → API key permissions) — ключ с выводом
  только с привязкой IP сервера.
- Права ключа: Withdraw (код `wwow` в `authorities`).
- Статусы (`GET /api/v2/spot/wallet/withdrawal-records`, «10 times/1s (User ID)»): `pending` («pending preliminary
  review»), `fail`, `success`. Поиск **по своему id**: параметр `clientOid` (или `orderId`) — «when passed, other
  parameters are ignored», хотя `startTime`/`endTime` помечены обязательными — передавать оба. `tradeId` — хеш.
- Откуда списывается вывод (спот или фонд) и **входит ли комиссия в `size` или сверху** — в документации нет → вопрос
  Тиграну / проверка малым выводом.
- Отмена вывода `POST /api/v2/spot/wallet/cancel-withdrawal` — только в первичной проверке и при включённом в кабинете
  «Cancel Withdrawal»; нам не нужна.

**Депозит.**
- Адрес: `GET /api/v2/spot/wallet/deposit-address?coin=&chain=` («10 times/1s (User ID)»): `address`, `chain`, `coin`,
  `tag`, `url`. Кэш по (монета, сеть), запрос в предпроверке (урок 25).
- История: `GET /api/v2/spot/wallet/deposit-records` («10 times/1s (UID)»), `startTime`/`endTime` обязательны,
  `limit` ≤ 100, курсор `idLessThan`. **Поиска по хешу нет** — фильтр по окну времени и сравнение `tradeId` («TX ID.
  when `dest` is `on_chain`, it's the on chain hash value»). Окно — настройка (урок 7).
- Статусы депозита: `pending`, `fail`, `success`. **Статуса «предварительно зачислено» (как MEXC PRE_SUCCESS) в
  документации нет** → `withdraw_hold` не нужен, пока живой депозит не покажет обратное (урок 22).
- **На какой счёт зачисляется** — в документации Classic не сказано. Есть «Modify Deposit Account»
  `POST /api/v2/spot/wallet/modify-deposit-account` (`coin`, `accountType`: `spot` / `funding` / `coin-futures` /
  `usdt-futures` / `usdc-futures`; «Modify the auto-transfer account type of deposit»). Лучшие практики (legacy):
  Funding Account — «Deposits, withdrawals, inter-account transfers». → Варианты: (а) выставить зачисление на `spot`
  по монете этим методом (в кабинете или API), (б) после зачисления — перевод `POST /api/v2/spot/wallet/transfer`
  (`fromType=p2p` — «P2P/funding account», `toType=spot`, `clientOid` идемпотентен: «If you set duplicate clientOid, it
  will return the result of existing clientOid transfer»), как `venue.deposit_to_spot` у BingX. **Вопрос Тиграну —
  какой вариант;** проверить на первом депозите.

## 4. Ордера

`POST /api/v2/spot/trade/place-order` — «Spot → Trade → Place Order»
(https://www.bitget.com/docs/catalog/classic-spot-trade/classic-spot-trade#place-order), «10 requests/second/UID».
- `orderType=limit`, `force` = `gtc` / `post_only` / `fok` / **`ioc`** — IOC есть официально (урок 5). Всё равно
  проверить живым ордером, что пара его принимает (урок 30).
- `clientOid`: правило — «must match `^[0-9A-Za-z_:#\-+\s]{1,32}$`», «uniqueness check only applies to all pending
  orders» (Best Practices, https://www.bitget.com/legacy-docs/classic/best-practices). Новый на каждую попытку (урок 6).
  Дубль — код 43118 «clientOrderId duplicate» (пример в документации).
- Ответ — только `orderId`, `clientOid` (приём, не исполнение) → итог из `GET /api/v2/spot/trade/orderInfo`
  (`orderId` или `clientOid`, «20 times/1s (UID)»): `status` = `live` / `partially_filled` / `filled` / `cancelled`
  (в примере текущих ордеров встречается `new` — не из перечня), `priceAvg`, `baseVolume`, `quoteVolume`,
  `cancelReason`, `feeDetail` — **JSON-строкой**: `newFees` {`c` купон, `d` BGB, `r` из получаемой монеты, `t` всего,
  `totalDeductionFee`}; блок `BGB` — старый формат. Монета комиссии явно — в `GET /api/v2/spot/trade/fills`
  (`feeDetail.feeCoin`, `totalFee`, `deduction`; время там — **секунды**, у orderInfo — миллисекунды).
- IOC: «`live` → `canceled`», «IOC order partially filled then canceled by system» (Best Practices).
- История ордеров и сделок — только 90 дней (Get History Orders, Get Fills: окно ≤ 90 дней) → настройки, проверка до
  ордера (урок 7).
- Правила пары (`/api/v2/spot/public/symbols`): `status` (`online` / `gray` / `halt` / `offline`), `pricePrecision`,
  `quantityPrecision`, `quotePrecision` → шаги 10^−n; `minTradeUSDT` — минимум суммы («Minimum trading volume (USDT)»;
  для пары не к USDT — смысл не описан); `minTradeAmount`/`maxTradeAmount` — «obsolete»; `buyLimitPriceRatio` /
  `sellLimitPriceRatio` — фильтр цены (проверяем в `order_rules::ready`, урок 4); `maxLimitOrderValue`,
  `maxMarketOrderValue` — только в примере ответа, в таблице полей нет; `orderQuantity` — максимум открытых ордеров;
  `offTime`. Флагов «покупка/продажа через API» нет — только `status`.
- **Пробного ордера (test order) в Classic спот нет** (11 эндпоинтов раздела «Spot → Trade», такого нет; в SDK UTA
  тоже нет) → `venue.order_test_first` = да для Bitget — ошибка настройки (как Gate).
- Исход при таймауте неизвестен — поиск по `clientOid` (orderInfo, затем отмена `POST /api/v2/spot/trade/cancel-order`
  по `clientOid`).

## 5. Стакан

WebSocket Classic, канал «Depth Channel» (https://www.bitget.com/docs/classic/websocket/spot/public/Depth-Channel):
- Подписка `{"op":"subscribe","args":[{"instType":"SPOT","channel":"books5","instId":"BTCUSDT"}]}`.
- `books1` (10 мс), `books5`, `books15` (200 мс) — каждый кадр `snapshot` N уровней; `books` — все уровни: первый
  `snapshot`, далее `update`, поле `seq` («incrementing except during symbol maintenance»).
- Контрольной суммы (checksum) для `books` на странице Classic **нет**; у UTA в канале `books`/`books50` есть `pseq`
  («seq of the previous update … must be equal to the pseq of the following») — для Classic не описано. → Берём
  **`books15` или `books5` (снимки)**, как Gate/KuCoin; отсев неизменных снимков (решение 109). `books` + `seq` — только
  для окон (решение 125), с пересинхронизацией по REST.
- Лимиты (WebSocket intro): «300 connection requests/IP/5min, Max 100 connections/IP», «240 subscription
  requests/Hour/connection, Max 1000 channel subscription/connection», рекомендация < 50 каналов на соединение, «up to
  10 messages per second» (сверка 08.11: теперь на странице — за 5 с на соединение subscribe 50, unsubscribe 50, ping 10,
  login 2) → `venue.ws_symbols_per_conn` ≤ 50, `venue.ws_subs_per_sec` ≤ 10. **240 запросов подписки в
  час на соединение** — пересподписки ограничены: не дёргать соединения при том же наборе (урок 14).
- Пинг: строка `"ping"` каждые 30 с, ответ `"pong"`; сервер рвёт без ping 2 минуты.
- REST снимок: `GET /api/v2/spot/market/orderbook` (`limit` ≤ 150, `type` step0…step5, «20 times/1s (IP)»); с 17.10.2026
  не отдаёт rToken (журнал 2026-10, нам не важно).
- Журнал 03.11.2026: с 17.11.2026 каналы `trade` и `candle*` перестают слать `snapshot` — для окон (`trade`) учесть.

## 6. Комиссии

- По паре и ключу: `GET /api/v2/common/trade-rate?symbol=&businessType=spot` («10 times/1s (UID)»): `takerFeeRate`,
  `makerFeeRate` (дробью); все пары — `GET /api/v2/common/all-trade-rate?businessType=spot`
  (https://www.bitget.com/docs/catalog/classic-common-public/classic-common#get-trade-rate). Одним запросом на все
  пары — фоновый круг дешёвый (урок 11, 26).
- Базовая ставка пары — `takerFeeRate` в `/spot/public/symbols`; уровни VIP — `/api/v2/spot/market/vip-fee-rate`.
- **Скидка BGB:** `GET /api/v2/spot/account/deduct-info` (`deduct`: `on`/`off`), `POST /api/v2/spot/account/switch-deduct`.
  Размер скидки и учтена ли она в `trade-rate` — **в документации нет**. Факт — `feeDetail.newFees.d` (списано BGB) и
  `r` (из получаемой монеты). → Урок 23: расчёт по обычной ставке (худший случай), скидка — только при `deduct=on` и
  наличии BGB; монета комиссии — по факту ордера. Что при нехватке BGB — не описано (вопрос, как у Gate GT).

## 7. Лимиты запросов

- «The overall rate limit is 6000/IP/Min»; публичные — до 20 запросов/с; приватные — свой лимит на каждом эндпоинте
  (по UID или IP), считаются независимо (Classic → REST API).
- Превышение — HTTP 429 «Too Many Requests». **`Retry-After` в документации нет** → пауза `venue.rate_limit_pause_ms`
  (как Gate). Бан IP — не описан.
- Журнал 18.08.2026 «VIP Rate Limit Rules» (Classic Spot & Futures): «VIP 1 rate limit increased from 30/s to 60/s» —
  к каким эндпоинтам относится, пересказ не показал; страницу «Rate Limit» прочитать не удалось.
- Ключевые: ордер 10/с UID, orderInfo 20/с, вывод 5/с, справочник монет 3/с IP, `account/info` 1/с.

## 8. Приватный WS (поток аккаунта)

- `wss://ws.bitget.com/v2/ws/private`, вход `{"op":"login","args":[{apiKey, passphrase, timestamp, sign}]}`,
  `sign = Base64(HMAC_SHA256(timestamp + "GET" + "/user/verify"))` (WebSocket intro; SDK `ws/bitget_ws_client.py`).
- **Противоречие:** текст документации — «Unix timestamp in milliseconds, which will expire in 30 seconds», а пример
  Java в документации и официальный Python SDK (`int(round(time.time()))`) — **секунды**. Проверить живым входом и
  сообщить Тиграну (урок 2).
- Канал баланса спота (`account`, `instType=SPOT`) и ордеров (`orders`) — страницы прочитать **не удалось**
  (переадресация); имена, поля и наличие хеша депозита — проверить по документации/живому кадру перед кодом.

## 9. Счета (панель «Балансы», решение 144)

| Счёт | Эндпоинт |
|---|---|
| Спот | `GET /api/v2/spot/account/assets` (`coin` — одна монета → решение 113; `assetType` hold_only/all): `available`, `frozen`, `locked`, `limitAvailable` |
| Фонд (funding/P2P) | `GET /api/v2/account/funding-assets` (`coin`): `available`, `frozen`, `usdtValue`; «Sub-account requests are not allowed» |
| Итог по всем счетам | `GET /api/v2/account/all-account-balance` («1 times/1s»): `accountType` (spot, futures, funding, earn, bots, margin) + `usdtBalance` — только оценка в USDT |
| Боты | `GET /api/v2/account/bot-assets?accountType=` |
| Фьючерсы / маржа / Earn | Разделы «Futures → Account», «Margin → … Account», «Earn → Earn Account» — пути не прочитаны; можно показать итог из all-account-balance |

Источник: https://www.bitget.com/docs/catalog/classic-common-account/classic-common-account#funding-assets и
https://www.bitget.com/docs/catalog/classic-spot-account/classic-spot-account#get-account-assets .

## 10. Права ключа

- Права при создании (REST API → API key permissions): Read-Only, Trade, Transfer, Withdraw. Нужны: Trade (спот),
  Transfer (если депозит на funding), Withdraw (с IP). Без фьючерсов и маржи (урок 19).
- Проверка: `GET /api/v2/spot/account/info` («1 time/1s»): `authorities` — коды прав (`stow` спот-торговля, `wtow`
  перевод, `wwow` вывод; запрещённые для нас `coow`/`cpow` фьючерсы, `smow` маржа, `ttow` копитрейдинг), `ips` — белый
  список IP, `userId`, `parentId`. Имена прав — только из этого перечня (урок 29).
- Режим счёта (Classic/UTA) — `GET /api/v2/spot/account/upgrade-status` показывает статус перехода (`process` /
  `success` / `fail`); прямого «текущий режим» в Classic не нашёл → проверить вживую, что отвечает v2 на UTA-счёте.

## 11. Уроки чек-листа для Bitget

| # | Как у Bitget |
|---|---|
| 1–3 | Документация переехала (`/api-doc` → `/docs`), старые ссылки молча ведут на обзор UTA — в комментариях кода ссылки только на `/docs/catalog/...`. Противоречия уже есть: время входа WS (мс / с), `status` `new` вне перечня, «Withdrawal status» в депозитах. Живые записи — с сервера бота |
| 4 | `order_rules::ready`: шаги из precision, `minTradeUSDT`, фильтр цены `buy/sellLimitPriceRatio`, `status=online`; пробного ордера нет |
| 5, 30 | `force=ioc` официально есть; проверить живым ордером |
| 6 | `clientOid` ≤ 32, `^[0-9A-Za-z_:#\-+\s]$`, новый на попытку |
| 7 | orderInfo + fills, окно 90 дней — настройки |
| 8 | Адрес — `chains[].contractAddress`; имя сети BSC — из живого ответа |
| 9, 28 | Одна монета `?coin=`; размер полного ответа и gzip — замерить |
| 10, 21 | Минимум депозита, точность, кратность, tag, подтверждения — всё в справочнике; `extraWithdrawFee` — процент |
| 11, 23, 26 | trade-rate / all-trade-rate фоном с повтором; BGB — худший случай |
| 12–14 | `books15`/`books5` снимками, ≤ 50 каналов, ≤ 10 сообщений/с, **240 подписок/час/соединение** |
| 15 | 429 без Retry-After → пауза настройкой |
| 16 | Приватный WS есть, детали каналов не прочитаны |
| 17–18 | Ключ, секрет, passphrase — в секреты |
| 19 | Адресная книга вывода обязательна (код 11000), вывод только с IP из белого списка |
| 20 | Ядро и шлюз не меняются |
| 22 | «Предварительно зачислено» не описано → `withdraw_hold` не нужен (проверить) |
| 24 | spot + funding + итог all-account-balance |
| 25 | deposit-address кэш |
| 27 | «Счёт не открыт» — кодов в документации нет; проверить на funding/фьючерсах |
| 29 | `authorities` + `ips` из `account/info` |

## 12. Вердикт

**ПОДКЛЮЧАТЬ** (на Classic API v2, при классическом счёте). Есть спот, IOC, справочник с адресом контракта по сети,
минимумом депозита, точностью и кратностью вывода, вывод и депозит через API, поиск вывода по своему `clientOid`,
комиссии по ключу одним запросом, приватный WS.

**Сложность — средняя.** Похож на **KuCoin/OKX** (HMAC-SHA256 + Base64, passphrase, отдельные счета spot/funding,
перевод между ними), справочник — как Gate/Bybit (сети внутри монеты, одна монета запросом). Стакан — снимками N
уровней, как Gate `spot.order_book` / KuCoin `level2Depth50`.

Главные риски:
1. **Classic vs UTA.** Новые функции идут в UTA; если счёт переведут в UTA — неизвестно, работают ли v2 спот-эндпоинты.
   Нужен ответ Тиграна и живая проверка режима.
2. **Адресная книга вывода** — без занесения адреса в кабинете вывод не пройдёт (код 11000); для CEX ↔ CEX — каждый
   адрес депозита другой биржи тоже.
3. **Счёт зачисления депозита** не описан (spot или funding) — либо Modify Deposit Account → spot, либо перевод
   p2p → spot; проверить первым депозитом. Откуда списывается вывод и входит ли комиссия в `size` — тоже не описано.
4. **Не проверены вживую:** имя сети BSC, заполненность `contractAddress`, смысл `minDepositAmount = 0`, время входа WS
   (мс/с), каналы приватного WS.
5. **Лимит 240 подписок в час на соединение** и нет checksum у `books` — только снимки `books15`, пересподписки
   экономно.
