# HTX (бывший Huobi) — анализ для подключения к ARB (05.11.2026, без кода)

Задача: можно ли подключить HTX как биржу ↔ DEX (BSC) и биржа ↔ биржа по чек-листу
`docs/notes/cex-integration-lessons-2026-11-04.md` (30 уроков), контракт `src/gateway/contract.rs`, образцы — `venues/gate`, `venues/kucoin`.

## Источники и что удалось проверить

| Источник | Статус |
| --- | --- |
| https://huobiapi.github.io/docs/spot/v1/en — официальная документация Spot API (далее **[DOC]**, ссылки ниже — на разделы этой страницы) | Прочитана целиком (≈347 тыс. символов) через WebFetch. Цитаты короткие (ограничение инструмента), таблицы — пересказом |
| https://www.htx.com/en-us/opend/newApiPages/ — новый портал документации HTX | **Не открылся** (прокси отказал в доступе, WebFetch — нет разрешения). Расхождения нового портала со старой страницей **не проверены** |
| https://github.com/HuobiRDCenter/huobi_Golang — официальный SDK, коммит `ce16bfb` от 2026-04-12 (далее **[SDK]**) | Склонирован, модели и пути прочитаны |
| https://github.com/huobiapi/docs — исходник документации | Устарел (последний коммит 2021-12-27), не использовался |
| Живые ответы `api.huobi.pro` / `api-aws.huobi.pro` | **Недоступны** из этой среды (прокси) — ни одного записанного ответа нет. Всё помеченное «живая запись» — на боте |

---

## 1. Спот API, адреса, подпись

- Спот есть ([DOC] «Quick Start → Access URLs»):
  - REST: `https://api.huobi.pro`, `https://api-aws.huobi.pro` (рекомендован AWS Japan; прокси не рекомендуются).
  - WS рынка: `wss://api.huobi.pro/ws` (и `api-aws`); WS инкрементального стакана MBP: `wss://api.huobi.pro/feed`.
  - WS аккаунта и ордеров: `wss://api.huobi.pro/ws/v2`.
- Подпись ([DOC] «Quick Start → Authentication»): Signature V2 — HmacSHA256. Параметры в строке запроса:
  `AccessKeyId`, `SignatureMethod=HmacSHA256`, `SignatureVersion=2`, `Timestamp` (UTC, `YYYY-MM-DDThh:mm:ss`, действует 5 минут).
  Строка подписи: `METHOD\nhost(нижний регистр)\npath\nпараметры, URL-кодированные и отсортированные по ASCII, через &`;
  HMAC секретом → Base64 → параметр `Signature`. **Подпись идёт в query даже у POST** (тело JSON не подписывается).
- WS v2: `action=req`, `ch=auth`, `authType=api`, `accessKey`, `signatureMethod=HmacSHA256`, `signatureVersion=2.1`, `timestamp`, `signature` ([DOC] «Websocket Account and Order → Introduction»).
- Passphrase **нет**.
- [SDK] поддерживает также подпись Ed25519 (`internal/requestbuilder/privateurlbuilder.go`, `config` `Sign = "25519"`); в прочитанной [DOC] это не описано — для нас достаточно HMAC.
- Ответы: v1 — `{status: "ok"|"error", err-code, err-msg, data}`, v2 — `{code, message, data}` ([DOC] «API Access → Response Format»; [SDK] модели). Два формата → два разбора в `reply.rs`.
- Символ пары: нижний регистр слитно (`cakeusdt`) — [DOC] «Reference Data → Get all Supported Trading Symbol(V2)».
- Ордера и балансы требуют `account-id` спот-счёта — берётся из `GET /v1/account/accounts` (тип `spot`).

## 2. Справочник монет и сетей

Три эндпоинта ([DOC] «Reference Data»):

| Эндпоинт | Что даёт |
| --- | --- |
| `GET /v2/reference/currencies?currency=` («APIv2 – Currency & Chains») | Основной: по монете `chains[]`: `chain`, `displayName`, `baseChain`, `baseChainProtocol`, `isDynamic`, `depositStatus`/`withdrawStatus` (`allowed`/`prohibited`), `numOfConfirmations`, `numOfFastConfirmations`, `minDepositAmt`, `minWithdrawAmt`, `maxWithdrawAmt`, квоты вывода в день/год/всего, `withdrawPrecision`, `withdrawFeeType` (`fixed`/`circulated`/`ratio`), `transactFeeWithdraw`, `min/maxTransactFeeWithdraw`, `transactFeeRateWithdraw`; у монеты `instStatus` (`normal`/`delisted`). Есть фильтр одной монеты → `coin_networks_of` (решение 113) |
| `GET /v1/settings/common/chains?currency=` («Get Chains Information») | `chain`, `dn`, `ac` («address of chain»), `ct`, `default`, `dma`, `wma`, `wp`, `de`, `we`, `fc`, `sc`, `awt`/`adt`/`ao` (тег адреса), `ft`, тексты приостановки |
| `GET /v1/settings/common/currencys` | Настройки монет (`dma`, `wma`, `wp`, `de`, `we`, `cawt`, `fc`/`sc`…) — без сетей и без адреса контракта |

- **Сеть BSC:** название сети в [DOC] не приведено (в примерах только `ERC20`/`TRC20`/`OMNI` в `baseChainProtocol`, `trc20usdt` в `chain`). Как именно называется BSC у HTX (`chain` и `baseChainProtocol`) — **только из живого ответа**, в настройку `venue.bsc_network`. Не угадывать.
- **Адрес контракта токена — в документации НЕТ.** Ни в `/v2/reference/currencies`, ни в `/v1/settings/common/currencys` поля нет. В `/v1/settings/common/chains` есть `ac` с описанием «address of chain» (смысл неясен). В [SDK] у модели `ChainsV1` (`pkg/model/common/getchainsresponse.go`) есть поле `ca`, которого **нет в [DOC]**. Что из них (и есть ли вообще) адрес контракта BEP20 — проверить живой записью `/v1/settings/common/chains?currency=cake` (сравнить с адресом CAKE в BSC). Это блокер сопоставления по правилу CLAUDE.md «адрес контракта у биржи совпадает с адресом токена в пуле» (урок 8).
- Статус ввода/вывода — `depositStatus`/`withdrawStatus` (v2) или `de`/`we` (v1).
- Комиссия вывода — `transactFeeWithdraw` при `withdrawFeeType=fixed`; при `circulated` — диапазон `min/maxTransactFeeWithdraw`; при `ratio` — `transactFeeRateWithdraw`. Для расчёта (худший случай) при `circulated` — максимум; как точно считает HTX — в [DOC] не описано → вопрос Тиграну.
- Минимум вывода — `minWithdrawAmt`; точность — `withdrawPrecision` (знаков; «refused to withdraw if greater than this» — у `wp`).
- **Минимум депозита** — `minDepositAmt` прямо в справочнике (не нужен фоновый круг адресов, как у Gate). [DOC] к `dma`: меньше минимума — либо зачислится, когда накопится, либо «will not be accumulated and will never be credited». Какой вариант у конкретной монеты — из ответа не видно → считать «сумма ниже минимума = потеря» (урок 21). Семантика пустого/нуля — в [DOC] нет → `venue.deposit_min_absent_ok`.
- Memo/tag: `awt`/`adt` в v1 и `addressTag` в адресе депозита. Для BEP20 тег не ожидается; если `addressTag` не пуст — отказ (как Gate/KuCoin).
- Объём: `/v2/reference/currencies` без параметра — все монеты (тысячи) → разбор в `spawn_blocking`, кэш за `Arc` (урок 9).

## 3. Вывод и депозит

**Вывод** ([DOC] «Wallet → Create a Withdraw Request»): `POST /v1/dw/withdraw/api/create`, поля `address`, `amount`, `currency`, `fee`, `chain`, `addr-tag`, `client-order-id`. 20 раз / 2 с.
- **Белый список обязателен:** «Only support the existed addresses in your withdraw address list». Адрес добавляется **только в веб-интерфейсе** ([DOC] «Query withdraw address»: «user needs to add the withdrawal address from the Web UI first»). Проверить можно `GET /v2/account/withdraw/address?currency=&chain=` — для «Проверить подключение» и предпроверки (адрес кошелька DEX / депозита второй биржи должен быть в списке).
- `fee` — передаётся явно (ожидается равной комиссии из справочника).
- `chain` обязательно указывать: по умолчанию для USDT — ERC20 (!). Без `chain` монета уйдёт не в ту сеть.
- Свой id: `client-order-id`; повтор с тем же id возвращает прежний вывод (идемпотентность). Поиск: `GET /v1/query/withdraw/client-order-id`.
- Квоты: `GET /v2/account/withdraw/quota` (на раз, день, год, всего, остатки) — в предпроверку.
- Отмена: `POST /v1/dw/withdraw-virtual/{withdraw-id}/cancel`.
- Статусы вывода ([DOC] «Search for Existed Withdraws and Deposits»): verifying, failed, submitted, reexamine, canceled, pass, reject, pre-transfer, wallet-transfer, wallet-reject, confirmed («one confirmation completed»), confirm-error, repealed. Итог: `confirmed` — успех; `failed`/`canceled`/`reject`/`wallet-reject`/`repealed` — провал; `confirm-error` — неясно (как Gate FAIL — ждать, событие) → вопрос Тиграну.
- Отдельного запроса «вывод по id биржи» нет — `withdrawal_status` через `client-order-id` или список `/v1/query/deposit-withdraw?type=withdraw`.

**Депозит:**
- Адрес: `GET /v2/account/deposit/address?currency=` → `address`, `addressTag`, `chain` (20 раз / 2 с). Кэш по (монета, сеть) (урок 25).
- История: `GET /v1/query/deposit-withdraw?type=deposit&currency=&from=&size=(1–500)&direct=` — **фильтра по хешу нет**, поиск `tx-hash` в листании.
- Статусы депозита: unknown, confirming, **confirmed** («user is able to transfer and trade»), **safe** («user is able to withdraw»), orphan. То есть `confirmed` = торговать можно, вывести нельзя — **аналог MEXC PRE_SUCCESS** → контракт `withdraw_hold` обязателен (урок 22).
- Счёт зачисления: в [DOC] прямо не написано; по смыслу статуса `confirmed` (можно торговать) и отсутствия funding-счёта в спот-API у HTX — спот. **Подтвердить на живом депозите**; `deposit_to_trading` — ничего не делает, если подтвердится.

## 4. Ордера

`POST /v1/order/orders/place` ([DOC] «Trading → Place a New Order»), 100 раз / 2 с.
- Поля: `account-id`, `symbol`, `type`, `amount`, `price`, `source` (`spot-api`), `client-order-id`, `self-match-prevent`.
- **IOC есть:** `type = buy-ioc / sell-ioc` (лимит + IOC), также `buy-limit-fok / sell-limit-fok`. Стартовое правило «не исполнился — отменяем» закрывается типом (урок 5). Тип — это часть `type`, а не отдельный time-in-force.
- Свой id: `client-order-id` до 64 символов, буквы (регистр важен), цифры, `_`, `-`; уникален 8 часов (в таблице ошибок — 24 ч, противоречие → брать 24 ч, сообщить Тиграну). Новый на каждую попытку (урок 6).
- Итог ордера: `GET /v1/order/orders/{order-id}` или `/v1/order/orders/getClientOrder?clientOrderId=` (50 / 2 с): `field-amount` (исполнено, база), `field-cash-amount` (исполнено в котируемой), `field-fees`, `state` (created, submitted, partial-filled, filled, partial-canceled, canceling, canceled). Средняя цена = `field-cash-amount / field-amount` (своего поля нет).
  Монета комиссии — из `GET /v1/order/orders/{id}/matchresults`: `filled-fees`, `fee-currency`, `fee-deduct-currency` (пусто — в монете сделки; `ht`/`hbpoint` — списано ими), `filled-points`, `role`.
  **Срок:** отменённый ордер API «no longer queryable after being cancelled for 2 hours»; поиск по client-order-id — ордера за 8 ч. Итог читать сразу (урок 7, сроки — настройки).
- Правила пары: `GET /v1/settings/common/market-symbols` (или `/symbols`): `pp`/`ap` (точность цены/количества, знаков), `minoa`/`maxoa`, `lominoa`/`lomaxoa` (лимит-ордер мин/макс количества), `minov` (мин сумма), `smminoa`, `state` (`online`, `pre-online`, `suspend`, `offline`, `fuse`…). Шаг = 10^−precision (как Gate).
- Ошибки: `order-value-min-error`, `order-limitorder-amount-min-error`, `order-orderprice-precision-error`, `order-orderamount-precision-error`, `invalid.client.order.id`, `base-symbol-trade-disabled`.
- **Пробного ордера (test order) нет** в [DOC] → `venue.order_test_first` = да для HTX — ошибка настройки; `check_order` — только по `order_rules::ready`.

## 5. Стакан

[DOC] «Websocket Market Data»:
- `market.$symbol.depth.step0` на `/ws` — снимок до 150 уровней раз в 1 с. Медленно для нас.
- `market.$symbol.mbp.refresh.$levels` (5/10/20) на `/ws` — снимок раз ≈100 мс, все пары. **Кандидат №1** (как MEXC/KuCoin снимками; отсев одинаковых снимков — решение 109).
- `market.$symbol.mbp.$levels` на `/feed` — инкремент с `seqNum`/`prevSeqNum`: 5/20 уровней — потиково, но **только ~30 пар** («only supports the following symbols at this stage»); 150/400 — по снимкам 100 мс, все пары, могут приходить пустые обновления. Снимок — `req` (5/20/150). Разрыв `prevSeqNum` → пересинхронизация.
- `market.$symbol.bbo` — лучшая цена потиково.
- Кадры **сжаты GZIP** (двоичные) — распаковка на каждый кадр (урок 13: процессор).
- Пинг: сервер шлёт `{"ping": ts}` ≈ каждые 5 с, клиент — `{"pong": ts}`; два пропущенных — обрыв. WS v2 — `action:ping/pong` каждые 20 с.
- Лимиты: `req` — 1 на 100 мс на соединение; превышение — `bad-request` «429 too many request». **Лимит тем на соединение в [DOC] не указан** → вопрос Тиграну/замер, в `venue.ws_symbols_per_conn`, `venue.ws_subs_per_sec`.
- REST снимок: `GET /market/depth?symbol=&type=step0&depth=5|10|20` (по умолчанию 150 при step0).

## 6. Комиссии

- `GET /v2/reference/transact-fee-rate?symbols=a,b,c` ([DOC] «Trading → Get Current Fee Rate Applied to The User»): `makerFeeRate`, `takerFeeRate` (базовые), `actualMakerRate`, `actualTakerRate` (с учётом скидки, иначе равны базовым). Максимум символов в запросе и лимит частоты — в прочитанной [DOC] **не указаны** → вопрос/живая запись.
- Скидка: оплата комиссии HT или HB-баллами (point); [DOC] FAQ: при достатке HT/баллов «filled-fees is empty while filled-points has value»; включается пользователем. [SDK] даёт `POST /v1/account/fee/switch` и `GET /v1/account/overview/info` (переключение и монеты скидки) — в прочитанной [DOC] их нет (новый портал не проверен).
- По уроку 23: расчёт по `takerFeeRate` (худший случай), скидка — только если монета скидки есть на счёте; факт — `fee-deduct-currency`/`filled-points` из matchresults. При списании HT/баллами монета сделки не уменьшается — это меняет количество к выводу (как GT у Gate).

## 7. Лимиты запросов

- [DOC] «API Access → New Version Rate limit Rule»: лимит **по UID на эндпоинт** (все ключи UID вместе); у каждого эндпоинта — своё «N times/2s» (ордер 100/2с, детали ордера 50/2с, кошелёк 20/2с, `/v2/account/valuation` 3/1с, history 5/2с). Публичные — по IP.
- Заголовки ответа `X-HB-RateLimit-Requests-Remain`, `X-HB-RateLimit-Requests-Expire` — использовать для паузы.
- **Ответ при превышении для REST в [DOC] не описан** (нет HTTP 429, нет кода; есть только `frequent-invoke` у перевода фьючерсов и 1006 у условных ордеров). **Retry-After нет.** → пауза `venue.rate_limit_pause_ms` + заголовок Expire; точный код — по живой записи, вопрос Тиграну.
- WS v2: 50 запросов/с на соединение, 10 соединений на ключ, 100 запросов/с на IP; коды 4000 `too.many.request` / `too.many.connection`.

## 8. Приватный WS (поток аккаунта)

Есть: `wss://api.huobi.pro/ws/v2`, авторизация подписью (см. п. 1), без токена/listenKey.
- `accounts.update#2` — баланс и доступно вместе; первым кадром — снимок балансов (`changeType=null`); поля `currency`, `accountId`, `balance`, `available`, `changeType` (order-*, deposit, withdraw…), `accountType`, `seqNum`. Закрывает урок 16 (баланс для расчёта — из потока).
- `orders#$symbol` и `trade.clearing#$symbol#1` — сделки и отмены ордеров (итог без опроса; в сырой журнал).
- Без сжатия (в отличие от рыночного WS).

## 9. Счета (панель «Балансы», решение 144)

- `GET /v1/account/accounts` — все счета: `spot`, `margin`, `otc`, `point`, `super-margin`, `investment`, `borrow` (100 / 2 с).
- `GET /v1/account/accounts/{account-id}/balance` — строки `trade`/`frozen`/`loan`/`interest`/`lock`/`bank` (100 / 2 с). Свободно = `trade`.
- `GET /v2/account/valuation` — итог по всем платформам (3 / 1 с); `GET /v2/account/asset-valuation?accountType=`.
- `GET /v2/point/account` — баллы.
- Фьючерсы — отдельный API деривативов (`/v1/futures/transfer` только перевод спот↔фьючерсы); эндпоинты баланса фьючерсов в спот-[DOC] **нет** → в панели «Фьючерсы» либо отдельный URL деривативов (не проверено), либо через `/v2/account/valuation` по платформам. Earn — в [DOC] нет (только тип `investment`).
- Отдельного funding-счёта для депозита нет (см. п. 3).

## 10. Права ключа

- Права ([DOC] «Quick Start → Preparation»): Read, Trade (ордера и **переводы**), Withdraw. Нам — Read + Trade + Withdraw.
- IP: до 20 адресов; ключ без IP истекает через 90 дней (в Change Log 2022-06-14 другая формулировка — «после 90 дней бездействия»). Привязать IP сервера.
- Проверка через API: `GET /v2/user/api-key?uid=&accessKey=` — «used by the parent user to query their own API key information»: `permission` (примеры `trade,readOnly`, `readOnly,trade,withdraw`), `status` (`normal`/`expired`), `ipAddresses`, `validDays`. `uid` — из `GET /v2/user/uid`. Закрывает урок 29 (своя запись ключа).
- Права «фьючерсы/маржа» отдельным флагом в [DOC] не выделены (Trade общий) — запрет фьючерсов в ARB обеспечивается кодом (не вызывать), не правами ключа.

## 11. Уроки чек-листа — что важно у HTX

| Урок | HTX |
| --- | --- |
| 1–3 Документация, противоречия, записи | Два портала (старый huobiapi.github.io и новый htx.com/opend) — **новый не прочитан**. Противоречия уже есть: client-order-id 8 ч / 24 ч; срок ключа без IP. Живых записей нет — первым делом записать публичные ответы (`/v2/reference/currencies`, `/v1/settings/common/chains`, `market-symbols`, кадры WS) |
| 4 order_rules | `market-symbols`: pp/ap, lominoa/lomaxoa, minov, state; `type` buy-ioc/sell-ioc |
| 5 IOC | Есть (`buy-ioc`/`sell-ioc`). Урок 30 — проверить живым ордером, что принимается на каждой паре |
| 6 Свой id | `client-order-id` ≤ 64, [A-Za-z0-9_-], новый на попытку |
| 7 Итог ордера | Нет поля средней цены (считать); монета комиссии — отдельный запрос matchresults; отменённый виден только 2 ч |
| 8 Адрес контракта | **Блокер: в [DOC] нет.** Кандидаты `ac`/`ca` (`ca` — только в SDK). Название сети BSC — только из живого ответа |
| 9 Большой справочник | `/v2/reference/currencies` — все монеты; одна монета — `?currency=` |
| 10 Ввод/вывод | Всё есть; `chain` у вывода обязателен (умолчание USDT — ERC20); белый список адресов только через веб |
| 11 Комиссия | `/v2/reference/transact-fee-rate` по паре/аккаунту, `actualTakerRate` |
| 12 Стакан | `mbp.refresh.20` (100 мс, все пары) или `mbp.150` + `seqNum`; лимит тем на соединение не описан |
| 13 Память/CPU | GZIP каждого рыночного кадра — замер нагрузки (шаблон, п. 8) обязателен |
| 14 Пересподписка | Как MEXC (решение 131) |
| 15 REST-лимиты | По UID на эндпоинт + заголовки Remain/Expire; ответ превышения и Retry-After не описаны |
| 16 Поток аккаунта | Есть, `accounts.update#2` |
| 17–20 Фича, секреты, права, ядро | Ключ+секрет (без passphrase); ядро не меняется |
| 21 Мин. депозита | `minDepositAmt` прямо в справочнике; ниже минимума — возможная потеря |
| 22 Удержание вывода | **Нужно:** `confirmed` (торговать) ≠ `safe` (выводить) — `withdraw_hold` по депозитам в `confirmed` |
| 23 Скидка своей монетой | HT / HB-баллы; расчёт без скидки, факт из matchresults |
| 24 Все счета | `/v1/account/accounts` + balance по каждому; фьючерсы/earn — не в спот-API |
| 25 Адрес депозита | Кэш, предпроверка |
| 26 Комиссии фоном | Повтор через `venue.fee_retry_s` |
| 27 «Счёт не открыт» | Счёт `spot` может отсутствовать у нового UID — пустой список ≠ ошибка (код ответа — по живой записи) |
| 28 gzip/IPv4 | Рыночный WS всегда GZIP; IPv4/IPv6 в [DOC] не описан |
| 29 Права ключа | `/v2/user/api-key` — своя запись, `permission` |
| 30 IOC живым | Да, пробного ордера нет — только живой минимальный ордер |

## 12. Вердикт

**ПОДКЛЮЧАТЬ — условно**, после одной живой проверки: есть ли в справочнике адрес контракта BEP20 (`ac` или `ca` у `/v1/settings/common/chains`). Если адреса нет — **НЕ ПОДКЛЮЧАТЬ** (правило CLAUDE.md: сопоставление только по адресу контракта; по тикеру нельзя).

Всё остальное нужное есть: спот, BSC-вывод/депозит через API с `chain`, IOC, клиентский id, комиссия по паре, приватный WS баланса, минимум депозита в справочнике, проверка прав ключа.

**Сложность: средняя.** Своя схема (не Binance-подобная): подпись V2 в query, `account-id`, два формата ответа (v1 kebab-case / v2 camelCase), GZIP-WS с ping/pong-ts, итог ордера из двух запросов, `withdraw_hold` (confirmed/safe). Ближе всего к **Gate** (точности 10^−n, справочник с минимумом депозита, итог из полей ордера) и **MEXC** (удержание вывода PRE_SUCCESS ≈ confirmed, снимки стакана). Пробного ордера нет (как Gate).

### Главные риски
1. **Адрес контракта не документирован** — `ac`/`ca` не описаны (или `ca` только в SDK); без живой проверки сопоставление невозможно.
2. **Белый список вывода только через веб** — каждый адрес (кошелёк DEX, депозит BingX/MEXC/Gate…) Тигран добавляет руками; вывод на адрес вне списка — отказ. Плюс `chain` обязателен (умолчание USDT — ERC20).
3. **Депозит `confirmed` ≠ `safe`** — купленное с депозита можно продать, но вывести только после `safe`; без `withdraw_hold` сделка с выводом застрянет.
4. **Лимиты и ошибки не до конца описаны** — ответ на превышение REST и Retry-After нет, лимит тем на WS-соединение нет, максимум символов в `transact-fee-rate` нет; новый портал документации не прочитан.
5. **Итог ордера короткоживущий** — отменённый IOC виден 2 ч, комиссия и её монета — отдельным запросом matchresults; GZIP рыночных кадров — нагрузка на CPU (замер обязателен).

### Вопросы Тиграну (в docs/TASKS.md при старте работ)
- Открыть доступ к https://www.htx.com/en-us/opend/newApiPages/ и `api.huobi.pro` для записи ответов (сеть BSC, `ac`/`ca`).
- Комиссия вывода `withdrawFeeType=circulated` — брать максимум?
- Вывод `confirm-error` — ждать или провал?
- Стакан: `mbp.refresh.20` (100 мс снимками) или `mbp.150` с seqNum?
- Скидка HT/баллами — включать на счёте бота или нет?
