# BloFin — анализ перед подключением (без кода)

Claude, 05.11.2026. Основа — только официальная документация BloFin. Кода нет, git не трогали. Решения по открытым
пунктам принимает Тигран.

## Источники и насколько им верить

| Метка | Источник | Что там |
|---|---|---|
| [S] | https://docs.blofin.com/spot.html | Спот API: адреса, подпись, лимиты, справочник пар, стакан, ордера, приватный WS спота, коды ошибок |
| [D] | https://docs.blofin.com/index.html | Основная документация (фьючерсы + общие разделы): Account — балансы, перевод, **вывод, адрес депозита, история депозитов/выводов, GET Currencies**; User — GET API Key Info; Errors |

Разделы ниже названы так, как они названы в оглавлении сайта (Slate: якорь = заголовок раздела, например
`index.html#get-currencies`).

**Что не удалось проверить:**
- Страницы прочитаны через WebFetch (пересказчик, дословные цитаты ограничены) — каждую цифру и имя поля перед кодом
  сверить на живом ответе. Прямой `curl` к `docs.blofin.com` и `openapi.blofin.com` из рабочей среды — прокси 403.
- **Живых ответов API нет**: ни справочника пар (сколько спот-пар, есть ли BSC-токены), ни `GET /api/v1/asset/currencies`
  (как называется сеть BSC, нет ли в живом ответе недокументированного поля адреса контракта).
- Официальный GitHub BloFin (SDK Python, MCP-сервер) — доступ к репозиториям из сессии закрыт; не прочитан.
- Страница changelog не прочитана (в доступной части [D] записей нет).

---

## 1. Спот API, адреса, подпись

- **Спот есть**: отдельный раздел [S] «Spot API» (`instType = SPOT`, пары вида `BTC-USDT`).
- Адреса [S] «General Info»:
  - REST `https://openapi.blofin.com`;
  - WS спота: публичный `wss://openapi.blofin.com/ws/spot/public`, приватный `wss://openapi.blofin.com/ws/spot/private`
    (у фьючерсов [D] — `/ws/public`, `/ws/private`: у спота свои пути);
  - демо: `https://demo-trading-openapi.blofin.com`, `wss://demo-trading-openapi.blofin.com/ws/spot/{public,private}`.
  - Все — в настройки (`venue.rest_url`, `venue.ws_url`, `venue.ws_private_url`), без значений по умолчанию.
- Подпись [S]/[D] «REST Authentication»:
  - заголовки `ACCESS-KEY`, `ACCESS-SIGN`, `ACCESS-TIMESTAMP` (мс UTC), `ACCESS-NONCE` (уникальный на запрос, UUID),
    `ACCESS-PASSPHRASE`;
  - строка: `requestPath + METHOD + timestamp + nonce + body` (путь с query для GET; body — JSON без лишних пробелов
    для POST, пусто для GET);
  - **алгоритм необычный:** HMAC-SHA256(secret) → **hex-строка** → Base64 от байтов этой hex-строки
    («it is not hex2bytes, but rather string2bytes»). Это не как у OKX/KuCoin (там Base64 от сырых байтов) — тест на
    примере из документации обязателен.
  - Окно времени 60 с (код 152405), повтор nonce в течение 60 с запрещён (152407).
- **Passphrase есть** (задаётся при создании ключа; код 152408 «Passphrase error») — в файл секретов (урок 18).
- Ответ: `{code, msg, data}`, `code = "0"` — успех; HTTP 200 и для ошибок (кроме 429).
- Демо-торговля есть (отдельный хост) — можно гонять ордера без денег (но вывод/депозит в демо — не проверяемы).

## 2. Справочник монет и сетей

- `GET /api/v1/asset/currencies` [D] Account → «GET Currencies», без параметров (одной монеты — нет, урок 9 /
  решение 113: `coin_networks_of` — фильтр по полному списку).
- Поля (по таблице [D]): `currency`, `chain` («Chain name»), `depositMinAmount`, `depositUnsafeConfirmation`,
  `depositConfirmation`, `withdrawMinAmount`, `withdrawFee`, `withdrawPrecision` (Integer), `supportMemo` (0/1),
  `isDepositAvailable` (0/1), `isWithdrawAvailable` (0/1), `recoveryEtaDeposit`, `recoveryEtaWithdraw`, `logo`.
- **Адреса контракта токена в справочнике НЕТ.** Ни в таблице полей, ни в примере нет `contractAddress` / `contract` /
  `tokenAddress`; других эндпоинтов с адресом контракта (ни в [S], ни в [D]) нет.
  Это ломает жёсткое правило CLAUDE.md «адрес контракта у биржи совпадает с адресом токена в пуле» и урок 8: по
  решению 131 монета без адреса — «неподвижная», в расчёт не попадает. **Без адреса BloFin не даст ни одной рабочей
  монеты** (кроме BNB по решению 62 — она и так без адреса).
- Сеть BSC: как называется — **нет в документации** (в примерах только `TRC20`, `ERC20`). Значение — только из живого
  ответа, настройкой `venue.bsc_network` [blofin].
- Статус ввода/вывода: `isDepositAvailable` / `isWithdrawAvailable`; плюс `recoveryEta*` (когда откроют).
- Комиссия вывода `withdrawFee` (в монете вывода — подразумевается; `feeCurrency` есть в истории выводов), минимум
  `withdrawMinAmount`, точность `withdrawPrecision` (знаков; ошибка 152026 «Amount precision error»).
- Минимум депозита `depositMinAmount` — прямо в справочнике (не только в адресе депозита, как у Gate — урок 21 проще).
  Семантика null / «0» — **нет в документации**; `venue.deposit_min_absent_ok` — решение Тиграна.
- Memo/tag: `supportMemo`; в адресе депозита поле `tag`, в выводе параметр `tag`. Для BSC memo не ожидается, но
  проверять по `supportMemo` живого ответа.
- Подтверждения: `depositUnsafeConfirmation` и `depositConfirmation` — что значит каждое, [D] не объясняет (см. п. 3,
  урок 22).

## 3. Вывод и депозит через API

**Вывод — есть:** `POST /api/v1/asset/withdrawal-apply` [D] Account → «Withdrawal».
- Параметры: `currency`, `address`, `dest` (`onchain` / `internal`), `amount` (**без комиссии** — комиссию резервировать
  сверху, брать из GET Currencies), `chain` (обязателен для onchain), `addrType` (1 — адрес кошелька), `tag`,
  `clientId` (до 32 буквенно-цифровых, регистр важен). Ответ: `withdrawId`, `clientId`.
- **Только с funding-счёта**: «Only withdrawal of assets from the funding account is supported». Перед выводом —
  перевод spot → funding (`POST /api/v1/asset/transfer`, право `TRANSFER`), как `venue.withdraw_from_trade` у KuCoin.
- Субаккаунты («Common sub-accounts») выводить не могут — ключ должен быть основного аккаунта.
- **Право ключа на вывод — нет в документации.** Прав всего три: `READ`, `TRADE`, `TRANSFER` (последнее описано как
  «funding transfers between different accounts»); отдельного WITHDRAW нет, а раздел «Withdrawal» строки
  «Permission» не содержит. Какое право нужно — вопрос Тиграну / живая проверка (ошибка 152404 «…check the requestPath
  or API key permissions»).
- **Белый список / адресная книга** — в документации явно не описаны, но коды ошибок [D] «Errors» говорят, что они есть:
  `152020` «Address binding not found», `152023` «address is still within the 24-hour withdrawal lock period»,
  `152024` «can only withdraw to whitelist addresses», `152031` «address is not marked as verification-free»,
  `152032` «Quick withdrawal daily limit exceeded. Please complete 2FA verification», `152029` дневной лимит.
  Значит: адрес кошелька DEX / другой биржи надо заранее добавить в кабинете, пометить «без проверки», ждать 24 ч;
  есть дневной лимит «быстрого вывода». Где это настраивается — нет в документации API, вопрос Тиграну.
- Статус: `GET /api/v1/asset/withdrawal-history` — фильтры `withdrawId`, **`clientId`** (поиск по своей метке — есть),
  `txId`, `state`; состояния `0` waiting manual review, `2` failed, `3` success, `4` canceled, `6` kyt, `7` processing.
  Состояния `1` и `5` в перечне нет — на живом ответе проверить, не приходят ли.
  (В [S] у этого эндпоинта параметра `clientId` в таблице нет, в [D] есть — расхождение, проверить вживую.)

**Депозит:**
- Адрес: `GET /api/v1/asset/deposit-address?currency=&chain=` [D] «Get Deposit Address» — поля `currency`, `chain`,
  `address`, `tag`; «including previously-used addresses» (может вернуться несколько — выбирать по правилу, спросить).
  На какой счёт зачисляет — не сказано, выбора счёта в запросе нет.
- История: `GET /api/v1/asset/deposit-history` — фильтры `txId`, `depositId`, `state`; поля `confirm`, `type`
  (0 блокчейн, 1 внутренний); состояния `0` pending, `1` done, `2` failed, `3` kyt. Поиск по хешу — есть.
- **«Предварительно зачислено»** как отдельного статуса нет. Но в справочнике две цифры подтверждений
  (`depositUnsafeConfirmation`, `depositConfirmation`) — по аналогии с OKX/Binance это «зачислено после N_unsafe,
  выводить можно после N»; в документации **не сказано** — проверять на первом депозите (урок 22, `withdraw_hold`).
- **Счёт зачисления — нет в документации.** Раз вывод только с funding, вероятно депозит идёт на funding и нужен
  перевод funding → spot (`venue.deposit_to_spot`, как BingX). Проверить на первом депозите.

## 4. Ордера

- `POST /api/v1/spot/trade/order` [S] «Place Order»: `instType=SPOT`, `instId`, `side`, `orderType` ∈
  `market`, `limit`, `post_only`, **`fok`, `ioc`** (IOC — отдельный тип ордера, цена обязательна), `price`, `size`
  (в базовой монете; `targetCurrency` — только для market), `clientOrderId` (до 32 буквенно-цифровых; код 152009 —
  «numbers, letters, or underscores … 32»: подчёркивание допустимо? — расхождение, брать только буквы и цифры).
  Дубль своего ID — `102002` (урок 6: новый ID на каждую попытку).
- Итог: `GET /api/v1/spot/trade/order-detail` (по `orderId` или `clientOrderId` — поиск по своему ID есть) —
  `state`, `filledSize`, `filledAmount`, `averagePrice`, `fee`, `feeCurrency`; сделки —
  `GET /api/v1/spot/trade/fills-history` (`fillPrice`, `fillSize`, `fee`, `feeCurrency`).
  **Расхождение:** `feeCurrency` в примере — `"base_currency"` (перечисление), а не тикер; описание: «For maker sell
  orders of Spot … quote currency». Как понимать — только по живому ответу (урок 7).
  История: `orders-history` — конечные `canceled`, `filled`, `partially_canceled`; срок хранения истории — нет в
  документации.
- Правила пары `GET /api/v1/spot/market/instruments?instType=SPOT` [S] «GET Instruments»: `tickSize`, `lotSize`,
  `minSize` (в базовой монете), `maxLimitSize`, `maxMarketSize`, `state` (`live` / `suspend`). **Минимальной суммы
  (notional) — нет в документации.** Флагов «покупка/продажа через API» нет (только `state`).
- Ценовой коридор: `GET /api/v1/spot/trade/order/price-range` → `maxPrice`, `minPrice` (ошибки 102064/102065) —
  можно проверять в `order_rules::ready` (урок 4).
- **Пробного ордера (test order) нет** в документации. Для `check_order` — только свои правила + коридор цены; есть
  демо-хост, но это другой счёт.
- Отмена: `POST /api/v1/spot/trade/cancel-order` (1000 / 102068 — уже исполнен/отменён).
- Лимит ордеров: 30 запросов за 10 с на UserId; пачка ≤ 20 (152006).

## 5. Стакан

- WS [S] «WS Order Book»: `books` — снимок (`action: snapshot`) и изменения (`update`) каждые 100 мс, поля `seqId`,
  `prevSeqId` (у снимка `prevSeqId = "0"`): разрыв цепочки → пересинхронизация (урок 12). Глубина — **расхождение:**
  «400 levels» в описании канала и «books 200 levels» в правилах слияния. Контрольной суммы нет.
  `books5` — снимок 5 уровней каждые 100 мс при изменении топ-5 (удобно: как BingX/MEXC снимки, без локального
  стакана).
- Подписка: `{"op":"subscribe","args":[{"channel":"books5","instId":"X-USDT"}]}`; предел — **суммарная длина `args`
  ≤ 4096 байт** на запрос; числа подписок на соединение и соединений на IP — нет в документации; новые соединения —
  1 в секунду на IP. В настройки: `venue.ws_symbols_per_conn`, `venue.ws_subs_per_sec`.
- Пинг: строка `ping` → `pong`, если нет сообщений N < 30 с; без подписки/данных 30 с — сервер рвёт соединение
  (`venue.book_stale_ms` нужна осторожно: `books5` молчит при неизменном топе).
- REST: `GET /api/v1/spot/market/books?instId=&size=` (≤ 100 уровней на сторону), **без `seqId`** — для
  синхронизации `books` использовать снимок самого WS, не REST.

## 6. Комиссии

- **Эндпоинта ставки комиссии (по паре/аккаунту) нет** ни в [S], ни в [D] (фee-rate есть только у партнёрки —
  ставки для приглашённых, фьючерсы). Урок 11 («комиссия — из API, не числом») по API не закрывается: известна только
  фактическая `fee` из ордера. Вопрос Тиграну: ставка taker настройкой площадки (нарушает «данные площадок — только из
  API») или не подключать.
- Скидки своей монетой (у BloFin своей монеты нет) — **нет в документации**.

## 7. Лимиты запросов

- [D]/[S] «Rate Limits»: REST по IP — 500 за минуту (превышение → блок 5 мин) и 1500 за 5 мин (→ блок 1 час);
  торговые эндпоинты — 30 за 10 с на UserId. Превышение — код `429` «Rate limit reached…» (HTTP 429).
- **Retry-After — нет в документации**; пауза — на срок блока по документации (настройки), не слать дальше (урок 15).
- «Risk Control Restrictions»: HTTP 403 — файрвол, бан 5 мин; правила файрвола не публикуются.
- Весов по эндпоинтам нет — каждый запрос = 1.

## 8. Приватный WS (поток аккаунта)

- `wss://openapi.blofin.com/ws/spot/private`, вход `op: login` с `apiKey`, `passphrase`, `timestamp`, `sign`, `nonce`;
  подпись — `/users/self/verify` + `GET` + timestamp + nonce тем же HMAC→hex→Base64; срок — 1 мин. Ошибка `60009`.
  (Пример кода ставит `nonce = timestamp`, текст — UUID: расхождение.)
- Каналы: `spot-account` (баланс по монетам: `balance`, `available`, `frozen`, `spotAvailable`…, при событиях и
  периодически) → `MarketEvent::Balance`; `orders` (состояние ордера, `filledSize`, `averagePrice`, `fee`,
  `feeCurrency`). Каналов депозита/вывода нет — статусы перевода только REST.
- listenKey не нужен (вход подписью на соединение).

## 9. Счета (панель «Балансы»)

- Один эндпоинт `GET /api/v1/asset/balances?accountType=` [D] «GET Balance»: `funding`, `spot`, `futures`,
  `copy_trading`, `earn`, `inverse_contract`, `usdc_contract`. Поля `currency`, `balance`, `available`, `frozen`,
  `bonus`. `account_balances` — по одному запросу на счёт, ошибка — в error счёта (урок 24).
- Единый счёт: `GET /api/v1/account/config` → `accountLevel` (0 обычный, 1 spot, 2 spot futures, 3 multi currency —
  1–3 «unified»); для unified [D] говорит «use `futures`». Как при unified читать спот — неясно; в «Проверить
  подключение» показывать `accountLevel`, unified — вопрос Тиграну (как Classic/UTA у KuCoin).

## 10. Права ключа

- Права: `READ`, `TRADE`, `TRANSFER` ([D] «API Key Creation»). Нужны: TRADE (ордера), TRANSFER (spot ↔ funding),
  и то, что разрешает вывод — **не описано** (см. п. 3).
- До 20 IP; ключ без IP истекает через 90 дней — для бота IP сервера обязателен (152406 — IP не в белом списке).
- Проверка: `GET /api/v1/user/query-apikey` → `readOnly` (0/1), `type` (1 — «Transaction», 2 — сторонний), `ips`,
  `expireTime`. **Отдельных флагов прав (TRADE/TRANSFER/вывод) в ответе нет** — «Проверить подключение» может
  показать только «чтение/запись», IP и срок; право TRANSFER проверяемо лишь пробным переводом.
- Ключ брокера (`brokerId`) не нужен — тип 1 «Transaction» (152011).

## 11. Уроки чек-листа против BloFin

| # | Урок | BloFin |
|---|---|---|
| 1–3 | Только документация, расхождения — на живом | Расхождения: 400/200 уровней `books`; `clientTransferId`/`clientId`; nonce в WS-входе; `feeCurrency` = `base_currency`; алфавит `clientOrderId`; `clientId` в истории выводов есть в [D], нет в [S]. Записать живые ответы на сервере бота |
| 4 | Одна функция правил, пробный ордер | Тест-ордера нет; правила — `tickSize`/`lotSize`/`minSize`/`maxLimitSize` + коридор `price-range`; мин. суммы нет |
| 5, 30 | IOC | Есть `orderType = ioc` — закрывается; проверить на демо-хосте |
| 6 | Новый свой ID | `clientOrderId` ≤ 32, дубль 102002 |
| 7 | Итог ордера | `order-detail` по clientOrderId; монета комиссии — неясна (перечисление вместо тикера) |
| 8 | Сопоставление по адресу контракта | **Не закрывается: адреса контракта в API нет** |
| 9 | Большой справочник | `GET Currencies` без параметров; запроса одной монеты нет — фильтр по кэшу |
| 10 | Ввод/вывод, memo, счёт депозита | Флаги, комиссия, минимум, точность, `supportMemo` — есть; счёт депозита — не описан; вывод только с funding → перевод spot → funding обязателен |
| 11, 23 | Комиссия из API, скидка | **Ставки комиссии в API нет**; своей монеты скидки нет в документации |
| 12 | Стакан, номера, пинг | `books` с `seqId/prevSeqId` или `books5` снимки; предел 4096 байт `args`, 1 соединение/с; `ping`/`pong` < 30 с |
| 13–14 | Память, пересподписка | Как MEXC/KuCoin: `books5` — отсев одинаковых снимков (решение 109) |
| 15 | 429, Retry-After | 429 есть, Retry-After нет; блок 5 мин / 1 час — паузы настройками; 403 — бан 5 мин |
| 16 | Приватный WS | `spot-account` и `orders`, вход подписью |
| 17–20 | Готовая фича, секреты, права | passphrase в секретах; прав вывода нет в документации; ядро не меняется |
| 21 | Минимум депозита | `depositMinAmount` в справочнике; null — не описан |
| 22 | Удержание вывода | Отдельного статуса нет; `depositUnsafeConfirmation` vs `depositConfirmation` — проверить на первом депозите |
| 24 | Все счета | `asset/balances` по `accountType` |
| 25 | Адрес депозита в кэш | `deposit-address` по (currency, chain); может вернуть несколько адресов |
| 26 | Повтор круга комиссий | Круга нет (нет эндпоинта) |
| 27 | «Счёт не открыт» | Код такого ответа — нет в документации |
| 28 | gzip, IPv4/IPv6 | Не описано |
| 29 | Права из списка ключа | `query-apikey` — только `readOnly`, без имён прав |

## 12. Вердикт

**НЕ ПОДКЛЮЧАТЬ (сейчас).** Спот и вывод через API у BloFin есть, но:
1. **Нет адреса контракта токена** в справочнике монет/сетей (`GET /api/v1/asset/currencies`) — жёсткое правило
   CLAUDE.md и урок 8 не выполнимы; по решению 131 ни одна монета BloFin (кроме BNB) не станет рабочей. Обходной путь
   (ручная таблица «тикер → адрес») противоречит «данные площадок — только из API» — только решением Тиграна.
2. **Нет ставки комиссии в API** (урок 11).
3. **Право ключа на вывод и белый список адресов не описаны** в документации API, при этом коды ошибок показывают
   адресную книгу, 24-часовую блокировку нового адреса, пометку «verification-free» и дневной лимит быстрого вывода
   (иначе 2FA) — автоматический вывод под вопросом.
4. Счёт зачисления депозита не описан; вывод только с funding — нужны переводы spot ↔ funding в обе стороны.

Пересмотреть, если: в живом ответе `GET /api/v1/asset/currencies` окажется адрес контракта (недокументированное поле —
тогда спросить BloFin/Тиграна, можно ли на него опираться) или BloFin добавит его в документацию; плюс подтвердить
право на вывод.

**Сложность, если всё же подключать: средняя.** API — OKX-подобный (`instId` `BTC-USDT`, `{code,msg,data}`,
passphrase, `op: subscribe/login`, `books`/`books5` с `seqId/prevSeqId`, счета funding/spot) — ближе всего к KuCoin
(passphrase, перевод между счетами перед выводом/после депозита) и Gate (минимум депозита, адресная книга). Отличия:
подпись с промежуточной hex-строкой и `ACCESS-NONCE`; один запрос балансов на счёт; IOC — отдельный `orderType`.

## Вопросы Тиграну (в docs/TASKS.md при решении подключать)

- Б-1. Адреса контракта в API BloFin нет — не подключаем или (против правила) ручное сопоставление?
- Б-2. Ставки taker в API нет — не подключаем или ставка настройкой?
- Б-3. Какое право ключа нужно для `withdrawal-apply` (READ/TRADE/TRANSFER) и как в кабинете BloFin включить адреса
  «verification-free» / белый список для кошелька DEX и других бирж; дневной лимит быстрого вывода.
- Б-4. Счёт `accountLevel` (обычный/unified) — какой держать для бота.
