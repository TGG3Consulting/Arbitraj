# Phemex — анализ перед подключением (без кода)

Claude, 05.11.2026. Основа — только официальная документация Phemex. Кода нет, git не трогали. Решения по открытым
пунктам принимает Тигран.

## Источники и насколько им верить

| Метка | Источник | Свежесть |
|---|---|---|
| [D] | https://phemex-docs.github.io/ — исходники сайта: репозиторий `github.com/phemex-docs/phemex-docs.github.io`, коммит `98ded36` от 2026-03-30. Файлы `source/includes/_generic.md`, `_spot.md`, `_withdraw.md`, `_transfer.md`, `_transferV2.md`, `_uta.md`, `_contract.md` | Текущая документация (сайт собирается из этого репозитория) |
| [G] | `github.com/phemex/phemex-api-docs`, коммит `2f54a92` от 2024-06-17 (`Public-Spot-API-en.md`, `Generic-API-Info.en.md`) | Старее [D]; там, где расходится, верим [D]. Полезен таблицами полей справочника пар и валют |
| [C] | CCXT `python/ccxt/phemex.py` (master) | **Не официальный источник.** Phemex в [D] называет CCXT «authorized SDK provider» (`_generic.md`, «CCXT integration»). Используем только как подсказку, что проверять на живом ответе |

Ссылки на разделы ниже — якоря сайта https://phemex-docs.github.io/ (Slate: якорь = заголовок раздела; у повторяющихся
заголовков сайт добавляет суффикс `-2`, `-3` — поэтому рядом указан файл исходника [D]).

**Что не удалось проверить:** из рабочей среды Claude сайт https://phemex-docs.github.io/ (WebFetch не пустил), `api.phemex.com` и
`phemex.com` недоступны (прокси 403). Живых ответов API нет. Страница «Order spamming limitations»
(`https://phemex.com/user-guides/order-spamming-limitations`, ссылка из [D]) не прочитана. **Все пункты «проверить на
живом» — записывать на сервере бота.**

---

## 1. Спот API, адреса, подпись

- Спот есть: раздел «Spot REST API» и «Spot Websocket API» (`_spot.md`). Пары спота — с префиксом `s`: `sBTCUSDT`.
- Адреса ([D] `_generic.md`, #general-api-information):
  - REST `https://api.phemex.com`, WebSocket `wss://ws.phemex.com` («further URI or querystring not allowed»);
  - VIP (только для IP из белого списка Phemex): REST `https://vapi.phemex.com`, WS `wss://vapi.phemex.com/ws`;
  - testnet: `https://testnet-api.phemex.com`, `wss://testnet-api.phemex.com/ws`.
  - Все три — в настройки (`venue.rest_url`, `venue.ws_url`), без значений по умолчанию.
- Подпись ([D] #rest-request-header, #endpoint-security-type): заголовки `x-phemex-access-token` (ключ),
  `x-phemex-request-expiry` (Unix-**секунды**, «normally Now() + 1 minute»), `x-phemex-request-signature` =
  HMAC-SHA256(секрет, `URL Path + QueryString + Expiry + body`), регистр важен. Необязательный `x-phemex-request-tracing`
  (< 40 байт). Формат вывода подписи (hex) прямо не назван; пример в «User authentication» — 64 hex-символа → hex.
  Строка запроса подписывается без `?`, в том виде, как отправлена (пример 2 — повторяющиеся `ordStatus=…&ordStatus=…`).
- **Passphrase нет.**
- Ответ REST: `{code, msg, data}`, `code = 0` — успех; эндпоинты `/md/...` — другой формат `{error, id, result}`.
- HTTP 5XX: «the execution status is **UNKNOWN** and could be Succeed» → OutcomeUnknown (решение 64).
- Числа: масштабированные целые ([D] #price-ratio-value-scales): `…Ep` — цена × 10^priceScale (у спота 8),
  `…Er` — доля × 10^8, `…Ev` — значение × 10^valueScale монеты; в выводе/депозите — `…Rv` (реальное значение строкой).
  Это главное отличие от всех сделанных бирж: масштаб каждой монеты (`valueScale`) — из справочника, ошибка масштаба =
  ордер в 10^k раз больше/меньше.

## 2. Справочник монет и сетей

- Пары и монеты: `GET /public/products` (и `/public/products-plus` — с датами листинга/делистинга в `timeline`) —
  спот в `.products[]` с `type=Spot`, монеты в `.currencies[]` ([D] #query-product-information). Полный список полей в [D]
  **не приведён**; таблицы полей — в [G] «All Spot trading products / currencies»: `baseTickSize(Ev)`, `quoteTickSize(Ev)`,
  `minOrderValue(Ev)`, `maxBaseOrderSize(Ev)`, `maxOrderValue(Ev)`, `defaultTakerFee(Er)`; у монет `valueScale`, `minValueEv`,
  `maxValueEv`, «need addr arg». Поля `status: "Listed"`, `needAddrTag`, `buyPriceUpperLimitPct`/`sellPriceLowerLimitPct`
  есть только в примере [C] → проверить на живом.
- Сети монеты — **по одной монете за запрос**, сводного справочника всех монет с сетями в документации нет:
  - депозит: `GET /phemex-deposit/wallets/api/chainCfg?currency=` (подписанный) ([D] #query-deposit-chain-settings):
    `chainName`, `chainCode`, `status` (Active/Suspend — ввод), `minAmountRv` (минимум депозита), `confirmations`,
    **`contractAddress`**;
  - вывод: `GET /phemex-withdraw/wallets/api/asset/info?currency=&amount=` ([D] #query-withdraw-chain-settings):
    `chainInfos[]` — `chainName`, `status` (Active/Suspend — вывод), **`contractAddress`**, `minWithdrawAmountRv`,
    `minWithdrawAmountWithFeeRv`, `withdrawFeeRv`, `receiveAmountRv`. `currency` помечен «NO» (необязательный) — что вернёт
    запрос без монеты (все монеты?), **в документации нет** → проверить на живом: от этого зависит, сколько запросов нужно;
  - публичный `GET /exchange/public/cfg/chain-settings?currency=` ([D] #query-chain-information) — **формат ответа не описан**.
- **Сеть BSC:** название `chainName` для BSC **в документации нет** (в примерах только BTC, ETH, TRX, EOS). [C] сопоставляет
  `BEP20 → "BNB"` — не факт. `venue.bsc_network` [phemex] — только из живого `chainCfg`/`asset/info`.
- **Адрес контракта:** есть, поле `contractAddress` у сети в обоих справочниках (в примере у ETH — `null`). Обязателен для
  сопоставления: нет адреса — монета не проходит первичный фильтр.
- Точность суммы вывода: **в документации нет** (есть `valueScale` монеты; в [C] у монеты `assetsPrecision`) — проверить.
- Минимум депозита: `minAmountRv` в `chainCfg`, поле необязательное, в примере `"0"`. Что значит null и «0» — **не сказано**
  → урок 21, `venue.deposit_min_absent_ok`.
- memo/tag: в адресе депозита поле `tag`; в выводе — `addressTag`; у монеты «need addr arg» [G] / `needAddrTag` [C].

## 3. Вывод и депозит

**Вывод** ([D] `_withdraw.md`, #deposit-and-withdraw-rest-api):
- `POST /phemex-withdraw/wallets/api/createWithdraw?currency=&address=&amount=&addressTag=&chainName=` — параметры в
  строке запроса, `amount` — строка реального значения. Ответ: `id`, `amountRv` («receive amount»), `feeRv`, `status`.
- Ключ: «Read & Trade & Withdraw permission», **IP обязателен**; вывод только с главного аккаунта.
- **Белый список обязателен:** «Only withdrawal address already added in the "withdrawal address list" on web would be
  allowed. No 2FA or email verification via API withdrawal.» → адрес кошелька бота (BSC) добавить в кабинете заранее.
- Статусы ([D] #withdraw-status): Success, Rejected, Security Checking, Security check failed, Pending Review, Address
  Risk, Expired («normally 30 minutes»), Cancelled, Pending Transfer. В примере ответа истории — `"Succeed"`, в таблице
  — `Success` → противоречие, проверить на живом.
- Отмена: `POST /phemex-withdraw/wallets/api/cancelWithdraw?id=`.
- **Свой id вывода (client id) — нет.** Статус одного вывода по id — **отдельного запроса нет**; только список
  `GET /phemex-withdraw/wallets/api/withdrawHist?currency=&chainName=&offset=&limit=&withCount=` и поиск в нём по `id`
  из ответа создания. Если создание оборвалось (таймаут/5XX) — найти вывод можно только по адресу + сумме + времени.
- Комиссия — вычитается из суммы или сверху: `asset/info` описывает `amount` как «withdraw amount with fee», есть
  `minWithdrawAmountWithFeeRv` и `receiveAmountRv` → похоже, комиссия **внутри** суммы, но прямо не сказано → проверить.
- С какого счёта уходит вывод (спот?) — **в документации нет** (`asset/info` отдаёт `balanceRv`/`allAvailableBalanceRv`
  без имени счёта).

**Депозит:**
- Адрес: `GET /phemex-deposit/wallets/api/depositAddress?currency=&chainName=` → `address`, `tag` ([D]
  #query-deposit-address-information). Есть и старый `GET /exchange/wallets/v2/depositAddress` (спот-раздел).
- История: `GET /phemex-deposit/wallets/api/depositHist?currency=&offset=&limit=&withCount=` — **фильтра по хешу нет**,
  ищем `txHash` в списке; данные за 3 месяца (старый `/exchange/wallets/depositList`).
- Статусы ([D] #deposit-status): New, Confirmed («Deposit confirmed»), Success («Succeed deposit to Phemex account»),
  Rejected, SecurityChecking, SecurityCheckFailed, AmlCsApporve. Аналога MEXC PRE_SUCCESS («можно торговать, нельзя
  вывести») **в документации нет**. «Зачислено» = `Success`; `Confirmed` — не зачислено до проверки на первом депозите.
- Счёт зачисления: в истории поле `depositTo` = `SPOT` или `Contract` (в таблице), но в примере его нет, а есть
  `accountType: 1` → противоречие. Как выбрать счёт зачисления — **в документации нет** (у адреса депозита нет параметра
  счёта). Если придёт на Contract — перевод `POST /wallets/account/transfer` (`fromAccType` CONTRACT → `toAccType` SPOT,
  обязательны и `amountEv`, и `amount`) ([D] #transfer-between-wallets-within-an-account). → `venue.deposit_to_spot`
  включать только после первого живого депозита.

## 4. Ордера

- Создание: предпочтительно `PUT /spot/orders/create?symbol=&clOrdID=&side=&qtyType=&baseQtyEv=&priceEp=&ordType=&timeInForce=`
  (параметры в строке запроса), есть и `POST /spot/orders` с телом JSON ([D] #place-order-http-put-prefered).
- **IOC есть:** `timeInForce` = `ImmediateOrCancel` (и `FillOrKill`) у `ordType=Limit` — напрямую в списке допустимых
  значений. Урок 30 (проверить, что принимает) — всё равно на первом живом ордере.
- Количество: `qtyType` `ByBase` (`baseQtyEv`) или `ByQuote` (`quoteQtyEv`). Бот — `ByBase` + лимит-цена.
- Клиентский id: `clOrdID`; отмена и поиск по нему: `DELETE /spot/orders?symbol=&clOrdID=`,
  `GET /spot/orders/active?symbol=&clOrDID=` (так в документации), `GET /api-data/spots/orders/by-order-id?symbol=&clOrdID=`.
  Длина и алфавит `clOrdID` — **в документации нет** (в примерах UUID). Ошибки дубля: 10001, 11085, 19999.
- Итог ордера: `GET /api-data/spots/orders/by-order-id` ([D] #query-orders-by-order-id-or-client-order-id): `avgPriceEp`,
  `avgTransactPriceEp`, `cumBaseValueEv`, `cumQuoteValueEv`, `cumFeeEv`, **`feeCurrency`**, `ordStatus`,
  `leavesBaseQtyEv`. Сделки: `GET /api-data/spots/trades` — `execBaseQtyEv`, `execPriceEp`, `execFeeEv`, `feeCurrency`,
  `feeRateEr`; окно по умолчанию — 2 суток, `limit` ≤ 200 (урок 7 — сроки в настройки). В таблице параметр написан
  `oderId` (опечатка) — проверить. Ответ создания тоже несёт `cumBaseQtyEv`, `cumFeeEv`, `avgPriceEp`, но со статусом
  `Created` — итог брать из запроса по id или из потока `wo`.
- Правила пары: `baseTickSizeEv` (шаг количества), `quoteTickSizeEv` (шаг цены), `minOrderValueEv`, `maxBaseOrderSizeEv`,
  `maxOrderValueEv` ([G] таблица, [D] #price-ratio-value-scales: «scaled qty must be multiple times of baseTickSizeEv»).
  Минимума количества отдельным полем нет — только минимум суммы. Ошибки: 11053/11055 цена, 11056/11058 количество,
  11114/11115 сумма.
- **Пробного ордера (test order) нет** в документации → `check_order` только по правилам (`order_rules::ready`).

## 5. Стакан

- WS `orderbook.subscribe` `["sBTCUSDT"]` — глубина 30, интервал 20 мс; `["sBTCUSDT", true]` — полный стакан, 100 мс
  ([D] #subscribe-orderbook-2). Сразу после подписки — снимок, потом `type: incremental` (qty 0 = удалить уровень), и
  каждые 60 с — контрольный снимок. Есть `sequence`, но **не сказано**, идут ли номера подряд по символу (можно ли ловить
  разрыв) → проверить на записанных кадрах; до проверки — пересинхронизация по 60-секундному снимку.
- **Отписка `orderbook.unsubscribe` снимает ВСЕ подписки стакана на соединении** (параметр пустой) — нельзя убрать
  один символ; смена набора = переподключение соединения (урок 14).
- Лимиты WS ([D] `_contract.md` «API Rate Limits», в спот-разделе [D] их нет, в [G] спот-раздел — те же): **до 5
  соединений на клиента, до 20 подписок на соединение, 20 запросов/с на соединение**; `wss://ws.phemex.com` — 200
  (подключений?) за 5 мин на IP ([D] #ip-ratelimits). Итого **≤ 100 стаканов**. Что такое «Client» (IP, ключ, UID) — не
  сказано → вопрос Тиграну/проверка. В настройки: `venue.ws_symbols_per_conn` ≤ 20, `venue.ws_max_conns` ≤ 5,
  `venue.ws_subs_per_sec` ≤ 20.
- Пинг: клиент шлёт `{"method":"server.ping"}` (или WS-ping) чаще 30 с, рекомендовано 5 с; тишина 3 интервала —
  переподключение ([D] #heartbeat-2).
- REST: `GET /md/orderbook?symbol=` (30 уровней), `GET /md/fullbook?symbol=` ([D] #query-order-book-2).
- Цены и количества в стакане — масштабированные целые (`priceEp`, размер в Ev базовой монеты).

## 6. Комиссии

- Ставка по паре через API: `GET /api-data/spots/fee-rate?quoteCurrency=USDT` → `symbolFeeRates[]` с `takerFeeRateEr`
  (÷10^8) по каждой паре ([D] #query-fee-rate-by-quote-currency). В таблице параметр назван `currency` — противоречие с
  форматом запроса, проверить. Есть ещё `defaultTakerFee` в справочнике (без учёта VIP).
- **Скидка своей монетой — в документации нет** (ни слова о токене Phemex или вычете комиссии). Монета комиссии — по
  факту `feeCurrency` ордера (урок 23: считаем по обычной ставке).

## 7. Лимиты запросов

([D] #rate-limits, `_generic.md`)
- IP: 5000 запросов за 5 мин; превышение — **блок IP на следующие 5 мин**.
- По пользователю, окно 1 мин, группы: `SpotOrder` 500/мин (создание, изменение, отмена, открытые ордера; отмена — вес 2),
  `Contract` 500/мин, `Others` 100/мин (всё остальное: балансы, справочники сетей, вывод, депозит, история; `kline` — 10).
- Превышение — **HTTP 429** и заголовок `x-ratelimit-retry-after-<group>` (секунды); также `x-ratelimit-remaining-*`,
  `x-ratelimit-capacity-*`. Для группы Others суффикс пустой.
- Следствие: справочник сетей **по одной монете** + 100/мин в Others → 300 монет × 2 справочника ≈ 6 минут одного
  потребления лимита; круг обновления сетей нужно растягивать и делить квоту с балансами/выводами. Код 418 в документации
  не упоминается.

## 8. Приватный WS (поток аккаунта)

- Есть: на том же `wss://ws.phemex.com` — `user.auth` `["API", ключ, HMAC-SHA256(ключ + expiry, секрет), expiry]`,
  expiry ≤ сейчас + 2 мин ([D] #user-authentication-2); затем `wo.subscribe` ([D] #subscribe-wallet-order).
- Сообщения: снимок, потом incremental; `wallets[]` — `currency`, `balanceEv`, `lockedTradingBalanceEv`,
  `lockedWithdrawEv`; `orders.open/closed/fills` — `ordStatus`, `cumBaseQtyEv`, `cumFeeEv`, `feeCurrency`, `execFeeEv`,
  `execPriceEp`, `clOrdID`. listenKey не нужен. Событий депозита/вывода с хешем в потоке **нет** — только изменение
  баланса (как у MEXC: событие будит опрос REST).
- Earn — `wm.subscribe` (investment account).

## 9. Счета (панель «Балансы»)

| Счёт | Эндпоинт | Ссылка |
|---|---|---|
| Спот | `GET /spot/wallets[?currency=]` — `balanceEv`, `lockedTradingBalanceEv`, `lockedWithdrawEv` | [D] #query-wallets |
| Маржа | `GET /margin-trade/wallets[?currency=]` | [D] #query-wallets-2 |
| Фьючерсы coin-M | `GET /accounts/accountPositions?currency=BTC` | [D] #query-trading-account-and-positions |
| Фьючерсы USDT-M | `GET /g-accounts/accountPositions?currency=USDT` | [D] `_contract.md` |
| UTA | `GET /uta-biz/assets?currency=` | [D] #query-asset |
| Earn | только WS `wm.subscribe` — REST **нет в документации** | [D] #subscribe-investment-account |
| Фонд/funding | отдельного фондового счёта в документации нет (типы счетов: SPOT, CONTRACT, MARGIN, COPY_TRADE) | [D] `_transferV2.md` |

Фьючерсы по монетам — монеты списком в настройке (`venue.futures_balance_currencies`), как у KuCoin.

## 10. Права ключа

- По документации права: «Read & Trade» и «Read & Trade & Withdraw» (`_withdraw.md`, `_copytrade.md`); для вывода IP
  обязателен. Отделено ли право спота от фьючерсов — **в документации нет** (похоже, «Trade» общее) → урок 19 закрыть
  ключом нельзя, только на счёте фьючерсов не держать денег.
- **Запроса «информация о ключе» (права, IP, срок) в документации нет** → «Проверить подключение» может только сделать
  пробные вызовы: `/spot/wallets` (чтение), `fee-rate` (чтение), `asset/info` (вывод?), и вывести точную ошибку
  (HTTP 401/403, `code` 10500 «Api key not found»/«API Signature verification failed» — тексты из [C]).

## 11. Уроки чек-листа против Phemex

| # | Урок | Phemex | Как закрывать |
|---|---|---|---|
| 1 | Только по документации | Документация бедная: нет полного списка полей products, формата chain-settings, длины clOrdID, имени сети BSC | Вопросы в `docs/TASKS.md`; всё сверять с записанными ответами |
| 2 | Противоречия | Найдены: `Succeed`/`Success` у вывода; `depositTo` в таблице vs `accountType` в примере; `amount` vs `amountEv`; `oderId`; `clOrDID`; `currency` vs `quoteCurrency` у fee-rate; пример fullbook ведёт на `/md/orderbook`; WS-лимиты только в контрактном разделе | Каждое — проверить на живом и сообщить Тиграну |
| 3 | Тесты на записях | Живых записей нет (доступ закрыт) | Записать на сервере: products, chainCfg, asset/info, кадры WS |
| 4 | Одна функция правил, пробный ордер | Пробного ордера нет | `check_order` — только `order_rules::ready`: шаги `baseTickSizeEv`/`quoteTickSizeEv`, `minOrderValueEv`, `maxBaseOrderSizeEv`, `maxOrderValueEv`, масштаб Ev |
| 5 | IOC | Есть `ImmediateOrCancel` | Limit + IOC; остаток снимает биржа |
| 6 | Новый id попытки | `clOrdID`, формат не описан | UUID (как в примерах) — подтвердить на живом |
| 7 | Итог ордера и сроки истории | `by-order-id`: `cumBaseValueEv`, `cumQuoteValueEv`, `cumFeeEv`, `feeCurrency`; окно истории 2 сут., limit ≤ 200 | Сроки/лимиты — настройки, проверка до отправки |
| 8 | Сопоставление по адресу, имя сети | `contractAddress` есть; имя BSC не описано | `venue.bsc_network` из живого справочника |
| 9 | Большой справочник | products — один запрос; сети — **по монете** | Разбор вне потоков tokio; сети — фоном по монетам подписки, кэш за Arc |
| 10 | Ввод/вывод, memo, счёт зачисления | Статусы есть; memo — `tag`/`addressTag`; счёт — SPOT или Contract | `venue.deposit_to_spot` после живого депозита; перевод `/wallets/account/transfer` |
| 11 | Комиссия по паре | `fee-rate` по quote-монете | Фоновый круг, кэш |
| 12 | Стакан, лимиты WS, пинг | 30 уровней/20 мс, snapshot+incremental+sequence; 5×20; ping < 30 с | Снимок + изменения; разрыв sequence — проверить; лимиты в настройки |
| 13 | Минимум выделений | Числа — целые i64 со своим масштабом | Разбор в целые, без f64 и без копий уровней |
| 14 | Пересподписка | Отписка — только всех стаканов сразу | Смена набора = пересоздание соединения только для изменившейся пачки; фоновые круги не трогать |
| 15 | REST-лимиты, Retry-After | 429 + `x-ratelimit-retry-after-<group>`; IP-блок 5 мин | Пауза группы по заголовку; Others (100/мин) — самый узкий |
| 16 | Приватный WS | `user.auth` + `wo.subscribe` | Баланс для расчёта — из потока |
| 17 | Пять частей фичи | — | Как у всех |
| 18 | Секреты | ключ + секрет, passphrase нет | Зашифрованный файл |
| 19 | Права ключа | Нет отдельного права «только спот» в документации | Сообщить Тиграну; деньги на Contract не держать |
| 20 | Ядро не меняется | Масштабированные числа — внутри адаптера | Наружу — Decimal в типах ядра |
| 21 | Минимум депозита | `minAmountRv` в `chainCfg` (по монете), null/0 не объяснены | Фоном по монетам подписки; `venue.deposit_min_absent_ok` |
| 22 | Удержание вывода | Аналога PRE_SUCCESS нет в документации | `withdraw_hold` — неприменим до живого случая; `Confirmed` не считать зачислением |
| 23 | Скидка своей монетой | Нет в документации | Обычная ставка, факт `feeCurrency` |
| 24 | Все счета | Спот, маржа, фьючерсы (2 вида), UTA; Earn — только WS | `account_balances` по таблице раздела 9 |
| 25 | Адрес депозита в предпроверке | `depositAddress` по (монета, chainName) | Кэш по (монета, сеть) |
| 26 | Повтор круга комиссий | — | `venue.fee_retry_s` |
| 27 | «Счёт не открыт» | Кодов такого рода в документации нет | Проверить на пустом счёте |
| 28 | gzip, IPv4/IPv6 | Не описано | `venue.api_ip`, gzip — проверить на живом |
| 29 | Своя запись ключа, имена прав | Запроса о ключе нет | «Проверить подключение» — пробными вызовами |
| 30 | IOC на живом | IOC в документации есть | Подтвердить первым живым ордером |

Дополнительно (нового нет у других бирж):
- **Масштабированные целые** (`Ep/Ev/Er/Rv`) — новый слой перевода чисел; ошибка = деньги.
- **Вывод без client id и без запроса по id** — поиск «потерянного» вывода по адресу/сумме/времени (решение 64 —
  OutcomeUnknown, остановка).
- **Потолок 100 стаканов** (5 соединений × 20 подписок), если «Client» — это IP/ключ.

## 12. Вердикт

**ПОДКЛЮЧАТЬ — условно**, после записи живых ответов с сервера бота. Спот есть, вывод/депозит через API есть, сеть с
`contractAddress` в справочнике есть, IOC есть, приватный поток баланса есть. Блокирующих пунктов по документации нет,
но три надо подтвердить на живом до кода: (1) имя сети BSC и наличие `contractAddress` у BSC-токенов в `chainCfg`/
`asset/info`; (2) на какой счёт (SPOT/Contract) зачисляется депозит; (3) что значит «5 соединений на клиента» — это
предел числа монет.

**Сложность: средняя-высокая.** На Binance-подобные (MEXC, AsterDEX) не похож. Ближе всего к KuCoin/Gate по форме
(JSON-RPC-подобный WS `method`/`params`/`id`, ответ `{code,msg,data}`), но со своей особенностью — масштабированные
целые числа, справочник сетей по одной монете, узкий лимит Others 100/мин и бедная документация (много «проверить на
живом»).

### Вопросы Тиграну (в `docs/TASKS.md` при старте работы)
1. Разрешить запись живых ответов Phemex с сервера (products, chainCfg, asset/info BSC-токенов, кадры WS стакана).
2. Потолок ~100 монет на Phemex (если подтвердится) — устраивает?
3. Ключ Phemex «Read & Trade & Withdraw» не отделяет спот от фьючерсов (по документации) — согласен?
4. Адрес кошелька бота добавить в «withdrawal address list» Phemex (без этого вывод через API невозможен).
5. VIP-адрес `vapi.phemex.com` (нужен белый список IP у Phemex) — запрашивать?
