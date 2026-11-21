# OKX — анализ перед подключением (без кода)

Claude, 05.11.2026. Основа — официальная документация OKX API v5 https://www.okx.com/docs-v5/en/ (одна страница
~1,56 млн знаков; разделы ниже названы по её оглавлению: «Раздел → Подраздел»), журнал изменений
https://www.okx.com/docs-v5/log_en/, официальный SDK https://github.com/okxapi/python-okx (v0.4.4, `okx/utils.py`,
`okx/consts.py`) и справка OKX https://www.okx.com/en-us/help/trading-fee-rules-faq. Чек-лист — все 30 уроков
`docs/notes/cex-integration-lessons-2026-11-04.md`.

**Как читалось и чего не удалось проверить.** Из среды Claude `www.okx.com` напрямую закрыт (прокси 403), страница
документации прочитана через пересказчик кусками по 100 тыс. знаков — дословные цитаты помечены кавычками, остальное —
пересказ. **Перед кодом каждую цифру и поле сверить на живом ответе API с сервера бота** (как MEXC, решение 133).
Справочник монет (`/asset/currencies`) у OKX — только с ключом, поэтому живой записи нет ни одной. Не проверено:
точное имя сети BSC (`chain`), заполнен ли `ctAddr` полным адресом, размер ответа справочника, якоря разделов.

## 1. Спот API, адреса, подпись

- Спот есть: `instType=SPOT`, ордер с `tdMode=cash` («Non-Margin mode cash») — «Order Book Trading → Trade → Place order».
- REST: «REST: https://openapi.okx.com» (новый домен с 20.05.2026, журнал изменений «New REST API domain:
  openapi.okx.com»; SDK ещё ходит на `https://www.okx.com`). «Using openapi.okx.com will not work for the regions below»:
  пользователи app.okx.com (US/AU) — `us.okx.com`, my.okx.com (EEA) — `eea.okx.com` — «Overview → Production Trading
  Services». Адрес — настройкой; **какой домен у счёта Тиграна (сущность OKX для Армении) — вопрос Тиграну.**
- WebSocket: `wss://ws.okx.com/ws/v5/public`, `.../ws/v5/private`, `.../ws/v5/business` (тот же раздел). Порт 8443
  перестал принимать соединения 31.10.2026 (журнал изменений) — только 443.
- Подпись REST («Overview → REST Authentication → Signature»): заголовки `OK-ACCESS-KEY`, `OK-ACCESS-SIGN`,
  `OK-ACCESS-TIMESTAMP` (ISO 8601 UTC с миллисекундами, например `2020-12-08T09:08:57.715Z`), `OK-ACCESS-PASSPHRASE`.
  «pre-hash string of timestamp + method + requestPath + body», HMAC SHA256 секретом, Base64. GET — строка запроса
  входит в `requestPath`, тело пустое (SDK: `signature()` в `okx/utils.py`). Расхождение часов > 30 с — ошибка 50102.
- **Passphrase — да**, обязателен (задаётся при создании ключа) → в секреты (урок 18), как у KuCoin.
- Демо-торговля — заголовок `x-simulated-trading: 1` (не нужен нам).

## 2. Справочник монет и сетей

Источник — `GET /api/v5/asset/currencies` («Funding Account → REST API → Get currencies»), **требует подписи**,
6 запросов/с на UID, параметр `ccy` (одна или несколько монет через запятую) — запрос одной монеты есть (урок 9,
решение 113). Список — «currencies available to the account's KYC entity» (зависит от сущности счёта).

| Что нужно | Поле / как | Цитата |
|---|---|---|
| Сеть | `chain` | «Chain name, e.g. USDT-ERC20 , USDT-TRC20» — **имя сети содержит тикер** (`<ccy>-<сеть>`). Слова «BSC»/«BEP20» в документации **нет**; по образцу ожидается `CAKE-BSC`, но это **только по живому ответу** |
| Адрес контракта | `ctAddr` | «Contract address». Внимание: в ответе адреса депозита и «non-tradable assets» `ctAddr` — «Last 6 digits of contract address». Сопоставлять — только по `currencies`, и только если там полный адрес (проверить на живом ответе) |
| Статус ввода/вывода | `canDep`, `canWd` (`canInternal` — внутренний) | «The availability to deposit from chain / withdraw to chain»; `depEstOpenTime`/`wdEstOpenTime` — «if canDep is true, it returns ""» |
| Комиссия вывода | `fee` | «The fixed withdrawal fee Apply to on-chain withdrawal»; `minFee`, `maxFee`, `minFeeForCtAddr`, `maxFeeForCtAddr` — Deprecated |
| Минимум/максимум вывода | `minWd`, `maxWd` | «The minimum on-chain withdrawal amount of currency in a single transaction» |
| Точность вывода | `wdTickSz` | «number of digits after the decimal point… The withdrawal fee precision kept the same as withdrawal precision» |
| Минимум депозита | `minDep` | «The minimum deposit amount of currency in a single transaction» — **прямо в справочнике** (урок 21: фоновый круг адресов депозита ради минимума не нужен). null-семантика в документации **не описана** → `venue.deposit_min_absent_ok` |
| memo/tag | `needTag` | «Whether tag/memo information is required for withdrawal, e.g. EOS will return true» |
| Подтверждения | `minDepArrivalConfirm`, `minWdUnlockConfirm` | зачисление и «можно выводить» — разное число подтверждений (см. п. 3, урок 22) |
| Лимиты | `wdQuota`, `usedWdQuota`, `depQuotaFixed`, `usedDepQuotaFixed` | суточная квота вывода и фикс-лимит депозита в USD |

BNB: нативная монета, `ctAddr`, по всей видимости, пустой → решение 62 (сопоставление с WBNB настройкой). Не проверено.

## 3. Вывод и депозит

**Вывод** — `POST /api/v5/asset/withdrawal` («Funding Account → REST API → Withdrawal»), 6 запросов/с на UID:
`ccy`, `amt` (комиссия **не входит** в `amt` — списывается сверху), `dest=4` (в сеть), `toAddr`, `chain`,
`clientId` (до 32 знаков, буквы/цифры — свой id на попытку, урок 6), `rcvrInfo` («For the specific entity users… this
information is required» — Travel Rule). Ответ: `wdId`, `clientId`.
- **Белый список обязателен:** «The API can only make withdrawal to verified addresses/account, and verified addresses
  can be set by WEB/APP»; ошибка 58207 «Withdrawal address isn't on the verified address list». Кошельки DEX и адреса
  депозита других бирж (BingX, MEXC) — заранее вручную в кабинете OKX.
- «Only supported withdrawal of assets from funding account»; «Common sub-account does not support withdrawal» — ключ
  только основного счёта.
- Статус: `GET /api/v5/asset/withdrawal-history` (поиск по `wdId`, `clientId`, `txId`), 6/с. Состояния: ожидание
  `19` (мало на горячем кошельке), `17` (Travel Rule), `10`, `0`, `4/5/6/8/9/12` (ручная проверка), `7` (одобрено);
  в сети `1`, `15`, `16` («up to 24 hours due to local regulations»), `-3` (отмена); итог `-2` отменён, `-1` провал,
  `2` успех. Отмена — `POST /api/v5/asset/cancel-withdrawal` (только 0/17/19). Есть `GET /asset/deposit-withdraw-status`
  (1 запрос / 2 с) с `estCompleteTime`. WS-канал `withdrawal-info` (business, с логином).

**Депозит:**
- Адрес — `GET /api/v5/asset/deposit-address?ccy=` (6/с): `addr`, `tag`/`memo`/`pmtId` (если нужны), `chain`,
  `ctAddr` (последние 6 знаков), `selected`, **`to`: «6: Funding account 18: Trading account»**, «The users under some
  entity (e.g. Brazil) only support deposit to trading account».
- История — `GET /api/v5/asset/deposit-history` (6/с), параметр `txId`, поле `actualDepBlkConfirm`. Состояния:
  `0` ожидание подтверждений, **`1` «Deposit credited» — зачислено, но вывести нельзя** (аналог MEXC PRE_SUCCESS),
  `2` успех (можно выводить), `8` монета на паузе, `11` чёрный список адресов, `12` счёт/депозит заморожен, `13`
  перехват на уровне субсчёта, `14` KYC-лимит. WS-канал `deposit-info` (business, с логином).
- **Счёт зачисления — Funding (6)** у адреса с `to=6` → обязателен перевод Funding → Trading перед продажей и Trading →
  Funding перед выводом: `POST /api/v5/asset/transfer` (`type=0`, `from`/`to` = 6/18, `clientId`), «Only API keys with
  Transfer privilege», 2 запроса/с на UID+монету, ответ не значит «переведено» — проверять `GET /asset/transfer-state`
  (success / pending / failed). Схема — как у KuCoin (`venue.deposit_to_spot`, `venue.withdraw_from_trade`).

## 4. Ордера

- `POST /api/v5/trade/order`: `ordType=ioc` — «Immediately execute the transaction at the order price, cancel the
  remaining unfilled quantity» (урок 5 закрыт штатно); также `fok`, `limit`, `market`. `px` — для limit/ioc/fok;
  `sz` у спот-лимита — в базовой монете. 60 запросов / 2 с на UID + инструмент. Ответ — `sCode` (0 = успех), `sMsg`,
  `ordId`, `clOrdId`. `expTime` в заголовке — запрос не исполняется, если пришёл позже.
- Свой id: `clOrdId` — «A combination of case-sensitive alphanumerics, all numbers, or all letters of up to 32
  characters». Поиск по `clOrdId` возвращает только последний ордер с таким id → id новый на каждую попытку (урок 6).
- Итог: `GET /api/v5/trade/order` (60 / 2 с): `state` (live, partially_filled, filled, canceled, mmp_canceled),
  `accFillSz` (накоплено, в базовой монете), `avgPx`, `fee` («always negative» — списано), `feeCcy`, `rebate`.
  Сделки — `GET /trade/fills` (3 дня), `fee` отрицательна, `execType` T/M.
- **Сроки истории (урок 7):** `orders-history` — 7 дней, но «canceled incomplete orders are kept for only 2 hours»;
  `orders-history-archive` — 3 месяца без отменённых без исполнения. Итог IOC читать сразу; срок — в настройке.
- Правила пары: `GET /api/v5/public/instruments?instType=SPOT` (20 / 2 с): `tickSz`, `lotSz`, `minSz`, `maxLmtSz`,
  `maxMktSz`, `maxLmtAmt`, `maxMktAmt`, `state` (live, suspend, preopen, test), `ruleType`. Минимальной **суммы** в
  справочнике нет — есть ошибка 51020 «should meet or exceed the minimum order amount» (сверять на живом отказе).
  Ценовые пределы — 51137 и `GET /public/price-limit` (проверить, применимо ли к споту).
- **Пробного ордера для спота нет:** `POST /api/v5/trade/order-precheck` — только «Multi-currency margin mode and
  Portfolio margin mode». `venue.order_test_first` = да для OKX — ошибка настройки (как Gate).
- Приватный WS-канал `orders` (private): «Data will not be pushed when first subscribed».

## 5. Стакан

«Order Book Trading → Market Data → WS / Order book channel» (public):
- `books5` — снимок 5 уровней, «every 100 ms when there are changes» (без номеров; как снимки Gate/KuCoin).
- `bbo-tbt` — 1 уровень, раз в 10 мс при изменении.
- `books` — первый снимок 400 уровней + изменения каждые 100 мс; `action` snapshot/update; «The prevSeqId in the new
  message matches with seqId of the previous message», у снимка `prevSeqId = -1` → разрыв = переподписка (урок 12).
- `books-l2-tbt` (400) и `books50-l2-tbt` (50, 10 мс) — только VIP4+, иначе 64003.
- `checksum` устарел: «fixed to 0 and must no longer be used for integrity verification» (с 23.06.2026).
- REST снимок: `GET /api/v5/market/books` (`sz` до 400, 40 / 2 с, обновляется раз в 50 мс); есть `books-full`.
- Лимиты WS («Overview → WebSocket»): 3 подключения/с на IP; «'subscribe'/'unsubscribe'/'login' requests per
  connection is limited to 480 times per hour»; «the total length of multiple channels cannot exceed 64 KB»;
  30 соединений на канал на субсчёт; коды закрытия 4008/4009 «subscribed channels exceeds the maximum limit» — **числа
  каналов на соединение в документации нет** → `venue.ws_symbols_per_conn` подбирать по живому отказу 4009.
- Пинг: строка `ping` → `pong`, таймер N < 30 с; без данных > 30 с соединение рвётся.

## 6. Комиссии

- `GET /api/v5/account/trade-fee?instType=SPOT&instId=…` (5 / 2 с): «Specifying this parameter returns the correct
  applicable fee rates»; поля `taker`, `maker`, `level`, `feeGroup`. **Знак:** «positive number, which means the rate of
  rebate» → комиссия приходит отрицательной, в расчёт — модуль (урок 11; проверить на живом ответе).
- Скидки за OKB **нет**: «The platform currently doesn't offer a feature that lets you offset trading fees by holding
  OKB» (справка Trading Fee Rules FAQ). Урок 23 — неприменим.
- Монета комиссии — `POST /api/v5/account/set-fee-type`: «only effective for Spot»; `feeType` 0 — в получаемой монете
  (покупка уменьшает количество), 1 — всегда в котируемой. Текущее значение — `feeType` в `GET /account/config`.
  Сверять с `venue.fee_deduction` (как Gate), факт — `fee`/`feeCcy` ордера.

## 7. Лимиты запросов

- Публичные — по IP, приватные — по UID, торговые — по UID + инструмент; «1000 order requests per 2 seconds» на
  субсчёт; у VIP5+ — ещё лимит по fill ratio. Есть `GET /trade/account-rate-limit`.
- Превышение: код 50011 «Rate limit reached…» с HTTP 429; 50013 «Systems are busy» тоже 429; 50061 — лимит ордеров
  субсчёта; 50040 «Too frequent operations». **Retry-After в документации нет** → пауза `venue.rate_limit_pause_ms`.
- Ключевые: справочник/адрес/история депозита/вывод — 6/с; перевод — 2/с; баланс торгового — 10 / 2 с; комиссии —
  5 / 2 с; инструменты — 20 / 2 с.

## 8. Приватный WS баланса

Есть: `wss://…/ws/v5/private`, логин `{"op":"login","args":[{apiKey, passphrase, timestamp, sign}]}`, `timestamp` —
секунды Unix, подпись `timestamp + "GET" + "/users/self/verify"` («Overview → WebSocket → Login»; ошибки 60004, 60005,
60009). Каналы: `account` (по событию и регулярно; `cashBal`, `availBal`; только ненулевые) и `balance_and_position`
(«pushed when triggered by events such as filled order, funding transfer»). Это торговый счёт; фонд — каналы
`deposit-info`/`withdrawal-info` на business. Ключ слушателя не нужен (в отличие от listenKey MEXC/BingX).

## 9. Счета (панель «Балансы», решение 144)

- Торговый (единый): `GET /api/v5/account/balance` (10 / 2 с) — `cashBal`, `availBal`, `frozenBal`, `eq`. В режиме
  «Spot mode» отдельного фьючерсного счёта нет — фьючерсы, если бы были, живут в том же торговом счёте.
- Фонд: `GET /api/v5/asset/balances` — `bal`, `frozenBal`, `availBal` (только ненулевые).
- Earn: `GET /api/v5/finance/savings/balance` (Simple Earn), `GET /api/v5/finance/staking-defi/orders-active`.
- Итог: `GET /api/v5/asset/asset-valuation` (1/с) — `details`: funding, trading, classic (deprecated), earn.

## 10. Права ключа

- Права: «Read», «Trade» («place and cancel orders, funding transfer…»), «Withdraw». Нужны Read + Trade + Withdraw;
  перевод 6↔18 требует «Transfer privilege» — по описанию Trade это право покрывает (сверить на живом ответе).
- Проверка своего ключа: `GET /api/v5/account/config` — `perm` («The permission of the current requesting API key»),
  `ip`, `label`, `acctLv` (1 Spot, 2 Futures, 3 Multi-currency margin, 4 Portfolio margin), `uid`/`mainUid`
  (основной счёт, если равны — вывод с субсчёта невозможен), `kycLv`, `feeType` (урок 29).
- «Each API key can bind up to 20 IP addresses»; ключ без IP с правами trade/withdraw истекает после 14 дней простоя →
  привязать IP сервера бота обязательно.
- Ошибки: 50111–50114 (ключ/время/подпись/авторизация, HTTP 401), 50120 «doesn't have permission».

## 11. Уроки чек-листа для OKX

| # | Что важно у OKX | Как закрыть |
|---|---|---|
| 1–3 | Документация через пересказчик, `currencies` только с ключом | Записать живые ответы на сервере бота (публичные — instruments, books, WS; приватные — тесты `#[ignore]`) |
| 2, 30 | IOC штатный; пробного ордера нет | Перечень `ordType` не в справочнике пары — проверять живым ордером малой суммы |
| 4 | Правила: tickSz/lotSz/minSz/max*; минимальной суммы нет | `order_rules::ready`; 51020 → понятная причина; check_order без пробного ордера |
| 5 | `ioc` | `ordType=ioc`, `tdMode=cash` |
| 6 | `clOrdId` ≤ 32, буквенно-цифровой; поиск по нему — только последний | Новый id на попытку, без «-» и «_» |
| 7 | Отменённые без исполнения — 2 часа в истории | Итог сразу после ордера; сроки — настройки |
| 8 | Сеть `<ccy>-<сеть>`; `ctAddr` полный только в `currencies` | `venue.bsc_network` = суффикс сети (например «BSC»), сравнение `chain == ccy + "-" + суффикс` внутри адаптера (стык не меняется); смысл значения настройки у OKX — **подтвердить с Тиграном** |
| 9, 28 | Запрос одной монеты (`ccy=`) есть; размер полного ответа неизвестен | `coin_networks_of` с `ccy`; полный — `spawn_blocking`, gzip |
| 10 | Депозит на Funding, вывод только с Funding | Переводы 6→18 после зачисления и 18→6 перед выводом + проверка `transfer-state` |
| 11, 23 | Знак ставки отрицательный; OKB-скидки нет; `feeType` счёта | Модуль ставки; сверка `feeType` с `venue.fee_deduction` |
| 12–14 | `books5` (снимки) или `books` (seqId); checksum = 0 | Снимки → отдавать ядру только изменившиеся (решение 109); для `books` — разрыв seqId → переподписка |
| 15 | 50011/429, без Retry-After; 480 подписок/час на соединение | Пауза площадки; подписки пачками с запасом по часовому лимиту |
| 16 | Приватный WS с логином | `account` / `balance_and_position` |
| 17–18 | Ключ + секрет + passphrase | Зашифрованные секреты; «Проверить подключение» — `account/config` |
| 19 | Вывод только на verified-адреса | Описать в UI: адреса DEX-кошельков и депозитов других бирж — в кабинет OKX заранее |
| 21 | `minDep` в справочнике | Без фонового круга адресов; null → `venue.deposit_min_absent_ok` |
| 22 | Состояние депозита `1` — зачислено, но вывести нельзя | `withdraw_hold` по `minWdUnlockConfirm` / состоянию 1 |
| 24 | Funding, Trading, Earn, итог | `account_balances` по п. 9 |
| 25 | Адрес — `deposit-address` с `to` и `chain` | Кэш по (монета, сеть); брать адрес с нужным `to` (решение Тиграна: 6 или 18) |
| 26 | Комиссии с ключом | Повтор через `venue.fee_retry_s` |
| 27 | Пустой баланс фонда — пустой список, не ошибка | Пустой счёт |
| 29 | `perm` своего ключа в `account/config` | Проверять Read/Trade/Withdraw и `acctLv = 1`, `uid = mainUid` |

## 12. Вердикт

**ПОДКЛЮЧАТЬ**, сложность **средняя**. Есть спот, IOC, свой id ордера, вывод через API, депозит с историей по хешу,
приватный WS баланса, полный адрес контракта в справочнике (по документации), минимум депозита в справочнике.

Похож не на Binance-подобные (MEXC, AsterDEX), а на **KuCoin**: passphrase, HMAC-SHA256 + Base64, ответ
`{code, msg, data}`, два счёта (Funding / Trading) с внутренним переводом. Формат стакана — смесь Gate (снимки
`books5`) и Binance-диффа (`books` с seqId/prevSeqId).

Условия до кода (вопросы Тиграну, `docs/TASKS.md`):
1. Какая сущность OKX и домен API у счёта (openapi / eea / us); доступен ли вывод из Армении по API, нужен ли `rcvrInfo`.
2. Счёт в «Spot mode» (`acctLv=1`) — подтвердить.
3. Сеть BSC в `chain` и полнота `ctAddr` — по живому ответу `currencies` с ключом.
4. Белый список: список адресов вывода (кошельки DEX из «Маппинга», депозиты BingX/MEXC по BSC) — внести в кабинет.
5. Адрес депозита: Funding (6) + перевод, или Trading (18), если сущность позволяет.
