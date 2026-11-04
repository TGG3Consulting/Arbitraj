# Kraken — анализ перед подключением (без кода)

Claude, 05.11.2026. Основа — официальная документация Kraken https://docs.kraken.com/api/ (сейчас она живёт под
https://docs.kraken.com/exchange/… и https://docs.kraken.com/api-reference/…), официальная OpenAPI-спецификация спот REST
https://docs.kraken.com/openapi/spot-rest.yaml (~680 тыс. знаков, прочитана целиком кусками по 100 тыс.) и справка
https://support.kraken.com. Чек-лист — все 30 уроков `docs/notes/cex-integration-lessons-2026-11-04.md`.

**Как читалось и чего не удалось проверить.** Из среды Claude `docs.kraken.com` и `api.kraken.com` напрямую закрыты
(прокси 403); страницы прочитаны через пересказчик — дословные цитаты в кавычках, остальное пересказ. Живого ответа API
нет ни одного. **Главное непроверенное: есть ли у Kraken сеть BSC для конкретных монет и как она называется** — это видно
только в живом ответе `GET /funding/v1/methods/{deposit|withdraw}` (нужен ключ с правом Funds — Query). Перед кодом
каждую цифру и поле сверить на живом ответе с сервера бота (как MEXC, решение 133). Официального SDK/спецификации на
GitHub Kraken проверить не удалось (сессия видит только свои репозитории; публичный `krakenfx/kraken-wsclient-py` — старый
WS v1, не используем).

Ссылки на разделы (далее по тексту — короткие имена):
- [AUTH] https://docs.kraken.com/exchange/guides/rest/authentication.md
- [RL-REST] https://docs.kraken.com/exchange/guides/rest/ratelimits.md
- [RL-TRADE] https://docs.kraken.com/exchange/guides/general/ratelimits.md
- [FUND] https://docs.kraken.com/exchange/guides/rest/funding.md — гид по Funding (Beta)
- [METHODS] https://docs.kraken.com/api-reference/funding-beta/list-funding-methods.md
- [WD-NEW] https://docs.kraken.com/api-reference/funding-beta/create-funding-withdrawal.md
- [WD-OLD] https://docs.kraken.com/api-reference/funding/withdraw-funds.md
- [DEP-OLD] https://docs.kraken.com/api-reference/funding/get-status-of-recent-deposits.md
- [OAS] https://docs.kraken.com/openapi/spot-rest.yaml — схемы `add`, `open`, `info`, `pairs`, `balanceex`, `deposit`,
  `withdrawal-3`, `methods-4`, `info-5`, пути `/funding/v1/*`, `/private/*`, `/public/*`
- [WS] https://docs.kraken.com/exchange/api-reference/spot-websocket.md
- [WS-INTRO] https://docs.kraken.com/exchange/guides/websockets/introduction.md
- [WS-BOOK] https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/book.md,
  чек-сумма — https://docs.kraken.com/exchange/guides/websockets/book-checksum-v2.md (сам гид не прочитан)
- [WS-BAL] https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/balances.md
- [WS-EXEC] https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/executions.md
- [WS-AUTH] https://docs.kraken.com/exchange/guides/websockets/authentication.md
- [LOG] https://docs.kraken.com/exchange/changelog.md
- [SUP-DEP] https://support.kraken.com/articles/360000674066-cryptocurrency-deposit-statuses
- [SUP-HOLD] https://support.kraken.com/articles/360000389606-withdrawal-holds-on-kraken-com

## 1. Спот API, адреса, подпись

- Спот есть: `POST /0/private/AddOrder` [OAS], пары `GET /0/public/AssetPairs`. Маржа у Kraken есть (параметр
  `leverage`) — не используем, ордер без `leverage`.
- REST: `https://api.kraken.com/0` (публичные `/0/public/*`, приватные `/0/private/*`); новые Funding (Beta) — на
  `https://api.kraken.com/funding/v1/...` и `/funding/v2/...` (без `/0`, сервер переопределён в операции) [OAS].
- WebSocket v2: публичный `wss://ws.kraken.com/v2`, приватный `wss://ws-auth.kraken.com/v2` (бета — `beta-ws…`) [WS].
  WS v1 не используем.
- Подпись [AUTH]: заголовки `API-Key`, `API-Sign`; в теле `nonce` — «always increasing, unsigned 64-bit integer».
  `API-Sign` = «HMAC-SHA512 of (URI path + SHA256(nonce + POST data)) and base64 decoded secret API key», результат —
  Base64. Путь — от `/0/private`. В гиде есть проверочный пример (AddOrder, nonce 1616492376594) — взять в тест подписи.
  У Funding (Beta) заголовки `api-key`, `api-sign`, `api-nonce` [FUND]; как именно для них собирается строка подписи
  (путь `/funding/v1/...`, тело JSON, query у GET) — **в прочитанных страницах не описано**, вопрос; сверить живьём.
- **Passphrase — нет.** Есть необязательный `otp` (2FA на ключе) — ключ бота без 2FA (вопрос Тиграну, если он включит).
- **Nonce — ловушка:** nonce на ключ только растёт; параллельные запросы одним ключом из разных задач приходят не по
  порядку → «EAPI:Invalid nonce», повторные — временный бан [AUTH]. Есть «nonce window» (допуск, задаётся в кабинете,
  виден в `GetApiKeyInfo.nonce_window`). Нужен единый генератор nonce на ключ в адаптере и окно в настройках ключа.
- В журнале изменений [LOG]: «Announced upcoming authentication breaking change effective October 1st, 2025» — суть
  изменения в прочитанном не раскрыта; проверить на живом запросе.

## 2. Справочник монет и сетей

Два слоя, и **адрес контракта есть только в новом**:

| Что | Где | Комментарий |
|---|---|---|
| Монета, статус | `GET /0/public/Assets` [OAS] | Поля `aclass`, `altname`, `decimals`, `display_decimals`, `collateral_value`, `status` — «enabled, deposit_only, withdrawal_only, funding_temporarily_disabled». **Сетей и адреса контракта нет.** Имена монет свои (`XXBT`, `XETH`, `ZUSD`; `altname` = `XBT`) — тикер ≠ наш тикер |
| Сети монеты | `GET /funding/v1/methods/deposit` и `/withdraw` [METHODS] | Метод = «монета на сети». Поля: `method_id` (UUID), `method_name`, `asset {class,name}`, `minimum_amount`, `maximum_amount`, `fees {base, percentage, included, min, max}`, `network {network_id, network_name, contract_address, on_chain_asset_symbol}`. Право «Funds permissions — Query». Фильтр `asset[class]`, `asset[name]`; страницы `limit` ≤ 10000, `cursor` |
| Адрес контракта | `network.contract_address` [METHODS], [FUND] (пример: USDC на Ethereum `0xa0b8…eb48`) | **Есть** — сопоставление по адресу возможно (урок 8). Заполнен ли для BSC-токенов и пуст ли у BNB — только живьём (BNB → решение 62) |
| Список сетей | `GET /funding/v1/networks` [OAS] | `network_groups {network_group_id, name, network_ids}`, `networks {network_id, name}`. Подтверждений и chain id нет |
| Сеть BSC | — | **В документации слов BSC/BEP20 нет** (примеры — Ethereum, Arbitrum, Avalanche C). Косвенно: инциденты Kraken «deposits on the Binance Smart Chain (BSC) network… All BSC-based tokens may experience delays» (27–28.10.2025, через агрегатор isdown.app — не официальный источник). Имя сети (`network_name`, вероятно «Binance Smart Chain» / «BNB Chain») и `network_id` — **только из живого ответа** → `venue.bsc_network` |
| Статус ввода/вывода по сети | — | **Отдельного флага у метода нет.** Метод есть в списке `deposit`/`withdraw` = направление доступно (так читается, явно не сказано); плюс `Assets.status` по монете целиком. Вопрос: пропадает ли метод из списка при остановке сети — проверить живьём |
| Комиссия вывода | `fees.base` (+ `percentage`, `min`, `max`) метода; точная — `GET /funding/v1/fees/{method_id}?amount=` → `fee`, `gross_amount`, `net_amount`, `withdrawal_fee_token` (5 минут) [OAS] | Комиссия может быть процентной — учесть в `withdraw_cost` |
| Минимум/максимум вывода | `minimum_amount`, `maximum_amount` метода; лимиты счёта — `GET /funding/v1/limits/withdrawal/{class}/{asset}` (`available_balance`, `maximum_amount`, `maximum_reason`, лимиты в USD/EUR и по числу попыток) | |
| Точность вывода | `Assets.decimals` по монете. Поля точности у метода вывода **нет** («withdrawal» — «Present only for withdrawal methods», полей нет) | Округление вниз по `decimals` — проверить живым выводом |
| Минимум депозита | `minimum_amount` у метода `deposit` [METHODS]; старый `DepositMethods.minimum` — «minimum net amount that can be deposited right now» | Прямо в справочнике — фоновый круг адресов (урок 21) не нужен. null-семантика не описана → `venue.deposit_min_absent_ok`. Плата за адрес: `deposit.address_setup_fee` — проверить, что для BSC её нет |
| memo/tag | В адресе депозита `address_details.crypto {address, tag, memo}` [OAS]; «If a tag or memo is present, include it» [FUND] | Для EVM ожидаемо пусто; непустой — отказ, как у Gate/KuCoin |
| Общий адрес | `deposit.shares_addresses_with_method_id` — токены делят адрес нативной монеты (ERC-20 = адрес ETH) [FUND] | Для BSC ожидаемо так же (не описано) |
| Подтверждения | — | **Нет в API.** На справке — отдельная статья по числу подтверждений |

Размер справочника: методы — пачками до 10000, отдельно по монете (`asset[name]`) — запрос одной монеты есть (урок 9,
решение 113). Разбор — вне потоков tokio, кэш за `Arc`.

## 3. Вывод и депозит

**Вывод — два варианта, оба с «белым списком»:**
- Новый [WD-NEW]: `POST /funding/v1/withdrawals`: `scope {method_id | network_id}` (по `network_id` — отказ, если у монеты
  на сети ≥ 2 метода), `address_id` — **id сохранённого адреса** (формат `AB…-…-…`), `amount {asset {class,name}, amount}`,
  `fee {fee_included, current_fee {max_fee} | quoted_fee {token}}`, `expected_address` (сверка адреса). Ответ:
  `withdrawal_id`, `net_amount`, `gross_amount`, `fee`, `approval_request_id` («Set when the withdrawal needs to be
  approved…» — вывод «припаркован», при отказе отменяется). Право «Funds permissions — Withdraw».
- Старый (deprecated, «will remain active but will no longer receive updates») [WD-OLD]: `POST /0/private/Withdraw`:
  `asset`, `key` — «Name of the withdrawal key, as set up on your account», `address` (сверка), `amount`, `max_fee`
  («EFunding:Max fee exceeded»). Ответ `refid`.
- **Белый список обязателен по устройству API**: выводить можно только на сохранённый адрес. Адрес можно создать через
  API `POST /funding/v1/addresses` (`scope`, `address_details.crypto {address, tag, memo}`, `name`) — «verified
  automatically», без письма, но нужно право «Add withdrawal addresses», о рисках которого Kraken предупреждает отдельной
  статьёй. Предлагаю: **адреса (кошельки DEX, депозиты BingX/MEXC/Gate) — вручную в кабинете, право добавления адресов
  ключу бота не давать** (решение Тиграна). Оговорка [FUND]: адреса, сохранённые на уровне сети/группы через API, «do not
  yet appear in the Kraken website withdrawal address book».
- **Свой id у вывода — нет** (в спецификации нет client id). Урок 6 закрывается иначе: id Kraken (`withdrawal_id` /
  `refid`) из ответа; при обрыве ответа — поиск по списку выводов (время, сумма, адрес) и остановка (OutcomeUnknown).
- Статусы: новый список `GET /funding/v1/withdrawals` — `pending`, `success`, `failed`; фильтра по `withdrawal_id` нет
  (только `asset`, `scope`, `status`, `start_time`/`end_time`, `cursor`, `limit` ≤ 500); поля `withdrawal_id`, `amount`,
  `fee`, `method_id`, `status`, `create_time`, `address_id`, `onchain_transaction`. Старый `WithdrawStatus` —
  `Initial, Pending, Settled, Success, Failure` + `status-prop`: `cancel-pending, canceled, cancel-denied, return,
  onhold`; `txid`, `network`. Отмена — `WithdrawCancel` (старый).
- **Задержка «подтверждение вывода» (`approval_request_id`)** — механизм не описан: кто одобряет и сколько ждать. Вопрос.

**Депозит:**
- Адрес: `GET /funding/v2/deposit/addresses` (выданные; `scope`, `asset`) и при отсутствии — `PUT /funding/v1/deposit/address`
  (`method_id`) → `address_details.crypto {address, tag, memo}`. Лимит неиспользованных адресов: `address_generation`
  `unlimited | limited (limit, обычно 5) | unsupported`, при пределе — `TooManyDepositAddresses`; неиспользованные
  истекают (`expire_time`) [FUND]. Кэш (урок 25) — с учётом `expire_time`.
- История: `GET /funding/v1/deposits` — `deposit_id`, `method_id`, `network_id`, `status`, `amount`, `fee`, `create_time`,
  `onchain_transaction`; **фильтра по хешу нет** — список по монете/методу/времени и поиск `onchain_transaction` у себя.
  Статусы «Settlement status»: `initial, pending, settled, success, failure` (смысл каждого не описан). Старый
  `DepositStatus`: `Initial, Pending, EarlyConfirmed, Settled, Success, Failure`, `status-prop`: `return`, `onhold`
  («on hold pending review»), `txid`, `originators` (депозиты, зачисленные свипом) [DEP-OLD].
- **«Предварительно зачислено» — есть** [SUP-DEP]: «Credited: …enough confirmations… to be credited» — торговать можно,
  выводить ещё нельзя; «Complete» — можно всё; «Not all cryptocurrencies use the Credited status». В API этому
  соответствует, по всей видимости, `EarlyConfirmed` старого API (соответствие **не написано** — проверить на живом
  депозите BSC). Урок 22: реализовать `withdraw_hold`. Источник суммы удержания в API не найден (у `BalanceEx` есть
  `hold_trade` — это ордера, не вывод). Вопрос: как узнать удержание вывода через API — `limits/withdrawal`
  `available_balance`? Проверить живьём.
- Удержания вывода [SUP-HOLD]: обычный крипто-депозит в причинах не значится; есть 24 ч на **новые** адреса после смены
  пароля; у метода депозита — флаг `exempted_withdrawal_hold`.
- Счёт зачисления: у Kraken спот-счёт один (кошелёк `spot`/`main` в потоке балансов [WS-BAL]); фондового счёта в
  документации нет, `WalletTransfer` — только «Spot Wallet» → «Futures Wallet». Депозит — на спот, `deposit_to_trading`
  = ничего (так читается; подтвердить первым депозитом). Новое: `ListWalletAccounts` (19.08.2026) и `account_id` у
  Funding — у счёта может быть несколько кошельков; какой кошелёк зачисляется по умолчанию — проверить.

## 4. Ордера

- Типы [OAS, схема `add`]: `ordertype` = market, limit, … ; **`timeinforce` = GTC, IOC, GTD, FOK** (FOK добавлен
  07–12.05.2026 [LOG]). Ордер: `limit` + `IOC` — урок 5 закрывается. Неисполненный остаток IOC — `expired` (в [WS-EXEC]
  `exec_type`/`order_status` = `expired`; явной связи с IOC в тексте нет — проверить пробным/живым ордером, урок 30).
- Свой id: `cl_ord_id` — «long UUID, short UUID, or free text… Free format ascii text up to 18 characters»; уникален
  среди **открытых** ордеров. Отмена по `cl_ord_id` есть (`CancelOrder`). `userref` — числовой, не уникальный. Новый
  `cl_ord_id` на каждую попытку (урок 6). Поиск закрытого ордера по `cl_ord_id` в `QueryOrders` — **не подтверждено** в
  прочитанном (по `txid` — да); вопрос.
- Итог ордера [OAS, `open`]: `status` (pending, open, closed, canceled, expired), `vol_exec` («Volume executed (base
  currency)»), `cost` (quote), `fee` («Total fee (quote currency)»), `price` («Average price»). В [WS-EXEC]:
  `avg_price`, `cum_qty`, `cum_cost`, `fees[] {asset, qty}` — «Fees are currently expressed in the quote currency only»,
  `liquidity_ind`. Монета комиссии управляется флагами `oflags`: `fcib` (комиссия в базовой) / `fciq` (в котируемой);
  по умолчанию — котируемая (USDT): покупка не уменьшает количество монеты — проще, чем Gate/KuCoin.
- Правила пары `AssetPairs` [OAS, `pairs`]: `status` (online, cancel_only, post_only, limit_only, reduce_only),
  `tick_size` («Minimum increment between valid price levels»), `lot_decimals` (знаки количества), `cost_decimals`,
  `pair_decimals`, `ordermin` (в базовой), `costmin` (в котируемой). Флагов «покупка/продажа через API» нет —
  `status = online` = торгуется.
- **Пробный ордер — есть:** `validate=true` — «the order will be validated only, it will not trade in the matching
  engine» → `check_order` при `venue.order_test_first` (урок 4). Проверяет ли он баланс — не сказано.
- Ордер через WS (`add_order` на ws-auth) тоже есть — можно позже, первая версия — REST.

## 5. Стакан

- WS v2 канал `book` [WS-BOOK]: `symbol[]` («BTC/USD»), `depth` ∈ {10, 25, 100, 500, 1000}, `snapshot` (по умолчанию
  да). Сначала `snapshot`, дальше `update` — **это дифф**, а не снимки N уровней (как у Gate/KuCoin). Целостность —
  `checksum` CRC32 по верхним 10 уровням в каждом сообщении; номера последовательности у `book` **нет**. Расхождение
  чек-суммы → пересинхронизация (отписка/подписка символа), с ограничением частоты (урок 12). Алгоритм чек-суммы — в
  [book-checksum-v2] (не прочитан, прочитать до кода; там форматирование цен/количеств без точки и ведущих нулей — нужна
  точность из `instrument`/AssetPairs). Урок 13: дифф держать в памяти без копий уровней.
- Сколько символов в подписке и подписок на соединение — **в документации не указано** («multiple symbols in one
  request», отдельный ack на символ) → `venue.ws_symbols_per_conn`, `venue.ws_subs_per_sec` — подобрать замером.
- Соединения: Cloudflare — «approximately 150 attempts per rolling 10 minutes» на IP, сверх — бан IP на 10 минут
  [WS-INTRO] → пауза переподключения в настройки. Сервер закрывает соединение «within approximately one minute of
  inactivity»; `ping` — «at least every 60 seconds» [WS]; есть канал `heartbeat`.
- Символы WS v2 — «BTC/USD» (не `XXBTZUSD` из REST): брать `wsname` из AssetPairs; сопоставить проверкой живьём.
- REST снимок: `GET /0/public/Depth?pair=&count=` (1–500, по умолчанию 100) — для окон записи (решение 125).

## 6. Комиссии

- Ставка по паре и ключу: `POST /0/private/TradeVolume` (`pair` — список пар; «Fees will not be included if pair is not
  specified») → `fees` (taker) и `fees_maker`, у каждой `fee`, `minfee`, `maxfee`, `nextfee`, … [OAS]. `fee_schedule=true`
  — вся сетка. **С 08.09.2026 `AssetPairs.fees`/`fees_maker` устарели и всегда `[]`** [LOG] — только TradeVolume.
  Счётчик REST +1 на вызов: пачками пар, фоном (урок 26 — повтор через `venue.fee_retry_s`).
- Скидки собственной монетой — **в документации нет** (у Kraken нет биржевой монеты типа BNB/GT/KCS). Урок 23 —
  неприменим; факт комиссии и монета — из ордера (`fee`, `fees[]`).

## 7. Лимиты запросов

- REST [RL-REST]: счётчик на ключ, +1 за вызов, +2 — Ledgers/TradesHistory. Потолок/убывание: Starter 15 / −0,33 в с;
  Intermediate 20 / −0,5; Pro 20 / −1. Сверх — «EAPI:Rate limit exceeded», ограничение «a few seconds», дольше — если
  продолжать слать. «EService: Throttled: [UNIX timestamp]» — слишком много одновременных, ждать до метки. **HTTP 429 и
  Retry-After в документации нет**: ошибки — в теле `error[]` (строки «EGeneral:Invalid arguments» и т. п.). Пауза —
  `venue.rate_limit_pause_ms`, а для Throttled — до метки из текста.
- Публичные REST — лимит **не описан** (в гиде — только приватный счётчик). Вопрос; на горячем пути REST нет (урок 15).
- Торговый движок [RL-TRADE], на пару: порог/убывание — Starter 60/−1, Intermediate 125/−2,34, Pro 180/−3,75; AddOrder
  +1; отмена — «+8 under 5 seconds» … «+1 under 300 seconds»; «EOrder:Rate limit exceeded», «EOrder:Orders limit
  exceeded». IOC сам снимается биржей — штрафа за отмену быть не должно (не сказано). Счётчик виден в `executions`
  (`ratecounter=true`).
- Уровень (Starter/Intermediate/Pro) зависит от верификации счёта — какой у Тиграна, вопрос; все числа — в настройки.
- Funding (Beta) — лимиты не описаны.

## 8. Приватный WS баланса

Есть: канал `balances` на `wss://ws-auth.kraken.com/v2` [WS-BAL], подписка с `token` из `POST /0/private/GetWebSocketsToken`
(право «WebSocket interface — On»; токен — использовать в течение 15 минут после создания, на каждое подключение новый
[WS], [WS-AUTH]). Снимок: `asset`, `balance`, `wallets[] {type spot|earn, id, balance}`; обновление на каждую проводку:
`ledger_id`, `ref_id`, `type` (deposit, withdrawal, trade, …), `amount`, `fee`, `balance` (итог), `wallet_type`,
`wallet_id`, `sequence`. Хеша транзакции в событии нет — событие будит проверку депозита по REST (как BingX/MEXC).
Соединение без приватной подписки закрывается — держать `balances` (+ `executions` для ордеров). Внимание: `balance`
— общий по кошелькам; для торговли нужен `spot`.

## 9. Счета (панель «Балансы», решение 144)

| Счёт | Эндпоинт | Комментарий |
|---|---|---|
| Спот | `POST /0/private/Balance` («net of pending withdrawals»), `POST /0/private/BalanceEx` (`balance`, `hold_trade`, `credit`, `credit_used`; доступно = balance + credit − credit_used − hold_trade) | Суффиксы `.B`, `.F` (устарел 13.11.2025), `.T`; `.S`, `.M` — только чтение. Баланса одной монеты нет — фильтр |
| Earn | `POST /0/private/Earn/Allocations` («balance in your earn account for each strategy»); в WS-снимке — кошельки `earn` | |
| Маржа (сводка) | `POST /0/private/TradeBalance` | Только показать |
| Фьючерсы | Отдельный API `https://futures.kraken.com/derivatives/api/v3` (свои ключи) | Только чтение; если не нужен — счёт с error «не настроен» |
| Кошельки счёта | `POST /0/private/ListWalletAccounts` (с 19.08.2026) | Типы кошельков в прочитанном обрезаны — живьём |
| Фондового счёта | нет | |

## 10. Права ключа

- Нужны: `query-funds` (Query), `add-funds` (Deposit — для старого DepositMethods/адресов), `withdraw-funds`,
  `query-open-trades`, `query-closed-trades`, `modify-trades`, `close-trades`, `create-ws-token`. Не нужны:
  `earn-funds`, `add-withdraw-address`/`update-withdraw-address` (риск, см. п. 3), `export-data`, `query-ledger` (если не
  берём Ledgers). Маржа отдельным правом **не выключается** — торговля и маржа под одним «Create & modify orders»;
  защищаемся тем, что не шлём `leverage`.
- Проверка: `POST /0/private/GetApiKeyInfo` (с 11.03.2026) — `api_key_name`, `permissions` (имена выше),
  `ip_allowlist`, `valid_until`, `nonce_window`, `last_used` [OAS]. Пример в спецификации — camelCase, схема —
  snake_case: разбирать по живому ответу (урок 2). «Проверить подключение» — права, IP, срок ключа, nonce window (урок 29).

## 11. Уроки 1–30 применительно к Kraken

| # | Важно? | Как закрывается |
|---|---|---|
| 1 | да | Ссылки на [OAS]/гиды в комментариях; непрочитанные места (подпись Funding Beta, чек-сумма) — прочитать до кода |
| 2 | **да** | Пример GetApiKeyInfo расходится со схемой (camel/snake); EarlyConfirmed ↔ «Credited» не связаны явно; IOC → `expired` — сверять на живых ответах |
| 3 | да | Публичное записать с сервера (Assets, AssetPairs, Depth, WS book); приватное — `#[ignore]` |
| 4 | да | `order_rules::ready`: `status=online`, `tick_size`, `lot_decimals`, `ordermin`, `costmin`; пробный — `validate=true` |
| 5 | да | `limit` + `timeinforce=IOC` |
| 6 | да | `cl_ord_id` ≤ 18 ASCII или UUID, новый на попытку. У вывода своего id нет — поиск по списку |
| 7 | да | Итог: `vol_exec`, `cost`, `fee`, `price`; сроки запросов — настройки |
| 8 | **да** | Адрес — `network.contract_address` метода Funding; тикеры Kraken свои (`XXBT`, `altname`, `wsname`) — сопоставлять только по адресу; имя сети BSC — живьём → `venue.bsc_network` |
| 9 | да | Методы по одной монете (`asset[name]`); полный список — фоном, `Arc` |
| 10 | да | Минимум/комиссия/лимиты — методы и `fees/{method_id}`; memo/tag — отказ; депозит на спот (проверить) |
| 11 | да | TradeVolume по парам (AssetPairs.fees устарели) |
| 12 | **да** | Дифф + CRC32 топ-10, без номеров; ресинк по чек-сумме с ограничением; пинг ≤ 60 с; лимиты подписок не описаны — замер |
| 13 | да | Дифф в памяти без клонов уровней |
| 14 | да | Как MEXC (решение 131) |
| 15 | **да** | Счётчик REST очень маленький (15–20 на ключ) → предпроверка перед ордером должна укладываться в несколько вызовов; 429 нет — ошибки в теле; Throttled — до метки |
| 16 | да | `balances` + `executions` на ws-auth, токен на подключение |
| 17 | да | Обычный набор пяти частей |
| 18 | да | Ключ + секрет (Base64) в секретах; passphrase нет |
| 19 | да | Без прав адресов; адреса вручную в кабинете; маржу правом не отключить |
| 20 | да | Ядро/шлюз не трогаем |
| 21 | частично | Минимум депозита в справочнике методов — круг адресов не нужен; null → `venue.deposit_min_absent_ok` |
| 22 | **да** | «Credited» (торговать да, выводить нет) → `withdraw_hold`; источник суммы в API — вопрос |
| 23 | нет | Монеты скидки нет; монета комиссии — `fcib`/`fciq` |
| 24 | да | Спот, Earn, TradeBalance, фьючерсы (отдельный API) |
| 25 | да | Кэш адреса по (монета, метод) с `expire_time`; лимит 5 неиспользованных — не плодить адреса |
| 26 | да | Как у остальных |
| 27 | возможно | Ответ «счёт/кошелёк не открыт» — не описан; смотреть живьём |
| 28 | да | Размер методов неизвестен; gzip — проверить; IPv4/IPv6 — `venue.api_ip` |
| 29 | да | `GetApiKeyInfo.permissions` — имена из спецификации |
| 30 | да | IOC и `expired` — живым/пробным ордером |

Плюс новое, чего не было у прошлых бирж: **единый nonce на ключ** (параллельные запросы ломают nonce), **дифф-стакан
по чек-сумме без номеров**, **вывод только по `address_id`**, **устаревший и новый Funding API одновременно** (берём
новый `/funding/v1`, старый — только `DepositStatus`, если `EarlyConfirmed` нужен для удержания).

## 12. Вердикт

**ПОДКЛЮЧАТЬ — условно**, после одного живого запроса `GET /funding/v1/methods/withdraw` и `/deposit` с ключом:
1. есть ли сеть BSC и для каких монет (если BSC-методов почти нет или только BNB — смысла нет, **НЕ ПОДКЛЮЧАТЬ**);
2. заполнен ли `contract_address` у BSC-методов;
3. сколько этих монет торгуются к **USDT** (у Kraken основная ликвидность альтов к USD/EUR; пары к USDT есть не у всех).

Всё остальное для нашего потока есть: спот, limit+IOC, свой `cl_ord_id`, пробный ордер `validate`, вывод и депозит
через API, адрес контракта (в новом Funding API), приватный WS баланса, проверка прав ключа.

**Сложность — средняя/высокая.** API не Binance-подобный (свои имена активов `XXBT/ZUSD`, тело form/JSON, nonce вместо
timestamp, ошибки строками в `error[]`, два Funding API). Ближе всего к **Gate** (HMAC-SHA512, справочник в двух
запросах) и **KuCoin** (сохранённые адреса/счета); стакан — свой (дифф + CRC32, как ни у одного из сделанных).

**Вопросы Тиграну:**
1. Доступен ли Kraken счёту (страна/верификация) и какой уровень (Starter/Intermediate/Pro) — от него лимиты.
2. Адреса вывода — вручную в кабинете (рекомендую) или дать ключу право «Add withdrawal addresses».
3. Можно ли с сервера бота сделать живую запись: методы deposit/withdraw (ключ Query), Assets, AssetPairs, WS book.
4. 2FA на ключе — без OTP (иначе бот не сможет подписывать).
