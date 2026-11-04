# AsterDEX (Aster) — анализ перед подключением

Claude, 05.11.2026. Только анализ, кода нет. Основа — официальная документация Aster. Каждый пункт сверен с чек-листом
`docs/notes/cex-integration-lessons-2026-11-04.md` (уроки 1–30). Решения по открытым вопросам принимает Тигран.

## Источники

| Метка | Источник | Что там |
|---|---|---|
| [S] | [GitHub asterdex/api-docs — Spot API V3 (EN)](https://github.com/asterdex/api-docs/blob/master/V3%28Recommended%29/EN/aster-finance-spot-api-v3.md), коммит `10391d1` от 03.11.2026 | Спот REST, WS, подпись, лимиты, ошибки |
| [C] | [Aster-Chain API (EN)](https://github.com/asterdex/api-docs/blob/master/V3%28Recommended%29/EN/aster-chain.md) | Депозит (контракт-хранилище), вывод спот/перп, история, перевод спот↔перп, стейкинг |
| [D] | [demo/aster-deposit-withdrawal.md](https://github.com/asterdex/api-docs/blob/master/demo/aster-deposit-withdrawal.md) | Справочник монет ввода/вывода (адрес контракта), оценка комиссии, лимиты вывода, история, подписи вывода |
| [F] | [Futures API V3 (EN)](https://github.com/asterdex/api-docs/blob/master/V3%28Recommended%29/EN/aster-finance-futures-api-v3.md) | Регистрация агента (API-кошелька) и его права; балансы перп-счёта |
| [O] | [Aster API Overview](https://github.com/asterdex/api-docs/blob/master/Aster%20API%20Overview.md), [README](https://github.com/asterdex/api-docs/blob/master/README.md), [CHANGELOG](https://github.com/asterdex/api-docs/blob/master/CHANGELOG.md) | V1 против V3, требование депозита (−5050), изменения |
| [A] | [demo/aster-code.py](https://github.com/asterdex/api-docs/blob/master/demo/aster-code.py) | Примеры: `GET /fapi/v3/agent`, `approveAgent`, `updateAgent` (в справочнике не описаны) |
| [W1] | [docs.asterdex.com — Spot Deposit & Withdrawal Guide](https://docs.asterdex.com/trading/spot/deposit-and-withdrawal-guide) | Пользовательское описание ввода/вывода спота (через WebSearch/WebFetch) |
| [W2] | [docs.asterdex.com — Spot Fee Structure](https://docs.asterdex.com/product/aster-spot/fees) | Ставки спота и скидка в ASTER |

**Что не удалось проверить:** живые ответы API. `sapi.asterdex.com` и `www.asterdex.com` из рабочей среды Claude закрыты
(прокси, 403). Корень `docs.asterdex.com` WebFetch не открыл. Поэтому каждую цифру нужно проверить на записанном ответе
с сервера бота: exchangeInfo, deposit/withdraw assets, estimateFee, depth и WS.

---

## 0. Что такое Aster и подходит ли наша модель биржи — коротко

- **Спот есть.** Это отдельный спотовый рынок, не перпетуалы: `https://sapi.asterdex.com/api/v3/*`, тип пары `SPOT`,
  статус `TRADING` ([S] «Spot API Overview», «Enumeration definition»). Перпетуалы — отдельное API `fapi`, его не трогаем.
- **API похож на Binance** (MEXC-подобный): `exchangeInfo`, фильтры `PRICE_FILTER/LOT_SIZE/NOTIONAL`, `depth`, `@depth`
  с `U/u`, listenKey, коды −1xxx/−2xxx, `X-MBX-USED-WEIGHT`. **Подпись другая:** не HMAC, а EIP-712 (ECDSA, secp256k1)
  ключом API-кошелька (агента), плюс `nonce` в микросекундах.
- **Счёт — это адрес EVM-кошелька (`user`).** Отдельного «адреса депозита биржи» в сети BSC у Aster нет:
  - **Ввод** — вызов контракта-хранилища на BSC `depositFor(currency, forAddress, amount, broker)`, `broker = 1000` → на
    спот, другое значение → на фьючерсы. Есть и второй способ: просто перевести токен на адрес хранилища — тогда деньги
    зачисляются **адресу отправителя** ([C] «Aster-Chain Deposit → EVM»; [D] «Deposit → EVM»).
  - **Вывод** — `receiver`: «The address of the current account» ([S] «Withdraw (USER_DATA)»), в подписи вывода
    `destination` — «should be the user's registered address» ([D] «EVM Withdraw Signature»). То есть вывод — только на
    свой же адрес. (Пользовательский гид [W1] пишет «your CEX deposit address or on-chain wallet address» — противоречие,
    проверять на живом выводе.)
- **Вывод для нашей схемы:**
  - **Биржа ↔ Aster (CEX ↔ CEX) — не получится.** Вывод BingX/MEXC на адрес хранилища Aster зачислится адресу
    горячего кошелька биржи (отправителю), а не нам. Вывод Aster → депозит BingX запрещён правилом «только на свой адрес».
  - **Aster ↔ DEX — получится, если счёт Aster = наш DEX-кошелёк.** Кошелёк из «Маппинга кошельков» сам является `user`
    в Aster. Ввод = **вызов контракта** `approve` + `depositFor(..., broker=1000)` из этого кошелька, а не обычный перевод
    ERC-20 на адрес. Вывод = перевод с Aster на этот же адрес.
  - Модель «биржа: адрес депозита → перевод → зачисление» применима только частично. Шаг «перевести монету на адрес
    депозита» у Aster — это вызов контракта. Сейчас DEX-кошелёк умеет только перевод. Нужна новая операция кошелька
    (подробнее — в п. 11, урок 20).

---

## 1. Спот API: адреса, подпись

| Что | Значение | Источник |
|---|---|---|
| REST спот | `https://sapi.asterdex.com` | [S] «Spot API Overview» |
| WS рынок | `wss://sstream.asterdex.com`: `/ws/<stream>`, `/stream?streams=a/b` | [S] «WebSocket market data feed» |
| WS аккаунта | `wss://sstream.asterdex.com/ws/<listenKey>` | [S] «WebSocket account information push» |
| Aster-Chain REST (ввод/вывод, перевод, стейкинг) | `https://chainapi.asterdex.com` | [C] «Aster-Chain API Overview» |
| Справочник монет ввода/вывода, оценка комиссии | `https://www.asterdex.com/bapi/futures/v1/public/future/aster/...` | [D] «Query APIs» |
| Перп (только для панели «Балансы») | `https://fapi.asterdex.com` | [F] «General API Information» |

**Подпись V3** ([S] «SIGNED Endpoint security», «Example of POST /api/v3/order»; [D] «Pro API-KEY Signature (V3)»):
1. К параметрам запроса добавляются `nonce` (микросекунды), `user` (адрес основного кошелька), `signer` (адрес
   API-кошелька).
2. Строка запроса (urlencode) становится полем `msg` типизированных данных EIP-712: домен
   `{name:"AsterSignTransaction", version:"1", chainId:1666, verifyingContract:0x0}`, тип `Message{msg:string}`.
3. Подпись ставится ключом API-кошелька и передаётся последним параметром `signature`.
4. Заголовков API-ключа нет. **Passphrase нет.**

- Nonce: окно ±60 с от времени сервера ([S] «V3 Nonce Mechanism»). Состояние ведётся на адрес агента, хранится
  **последние 100 nonce**. Новый nonce меньше минимального → «expired». Значит, параллельные запросы одного агента должны
  получать строго растущий nonce из одного счётчика.
  - Противоречие: [D] пишет «Nanosecond timestamp, valid within 30 seconds», регистрация агента ([F]) — «не больше 10 с».
    Проверить живьём.
- V1 (HMAC, `X-MBX-APIKEY`): новые ключи V1 с 25.03.2026 не создаются ([O] README). Делаем только V3.
- **С 01.09.2026 все подписанные V3-запросы работают только после депозита на основной кошелёк**, иначе
  `-5050 DEPOSIT_REQUIRED` ([O] README, CHANGELOG 2026-08-31).
- Все ответы — JSON. 4XX — ошибка запроса, 403 — WAF, 429 — лимит, 418 — бан IP, 5XX — сторона Aster ([S] «HTTP return
  codes»).

## 2. Справочник монет и сетей

- **Сеть BSC** задаётся числом `chainId = 56` (а не строкой вроде «BEP20»). Имя цепи в подписи вывода — `BSC`
  ([D] «chainName definition»). Для Aster настройка `venue.bsc_network` = `56`, а имя цепи для подписи — отдельная
  настройка. Значения берём из справочника, не зашиваем.
- **Адрес контракта токена есть:** `GET https://www.asterdex.com/bapi/futures/v1/public/future/aster/deposit/assets?chainIds=56&networks=EVM&accountType=spot`
  и такой же `.../withdraw/assets` ([D] «get all deposit assets», «get all withdraw assets»). Поля: `name`,
  `displayName`, `contractAddress`, `decimals`, `chainId`, `isNative`, `depositType` / `withdrawType`
  (`autoWithdraw`), `rank`.
  - Риск: это `bapi` сайта (домен `www.asterdex.com`), его нет в справочнике Spot API. Стабильность и лимиты этих
    адресов не описаны.
- В `exchangeInfo` адреса контракта **нет** (`symbols[]`: `baseAsset`, `quoteAsset`, фильтры, `orderTypes`,
  `timeInForce`). Связь «`name` справочника ввода/вывода = `baseAsset` пары» в документации не оговорена — проверить
  на живых ответах.
- Нативная BNB: в `depositFor` вместо адреса передаётся заглушка `0xfdAE1bA7C826aBDc4c99903c8056f82a1A04a615`
  ([C], [D]). В справочнике есть `isNative`. Сопоставление с WBNB — как решение 62, отдельной настройкой.
- **Статус ввода/вывода по сети.** Отдельного флага «ввод/вывод открыт» нет. Есть только присутствие монеты в списках
  deposit/withdraw assets. В счёте — общие флаги `canDeposit`, `canWithdraw` (`GET /api/v3/account`).
- **Комиссия вывода.** Её в запросе вывода (`fee`) указывает сам клиент.
  - Оценка: `GET /api/v3/aster/withdraw/estimateFee?chainId=56&asset=` → `gasCost` — «The minimum fee required for a
    withdrawal» в монете ([S] «Get withdraw fee»). То же есть в bapi `estimate-withdraw-fee` ([D]) и
    `GET /aster-chain/v3/withdraw/estimateFee` ([C]).
  - Ещё одно поле: `withdrawFee` по цепи в `user-withdraw-info` ([D], [C] «Get Withdraw Info»).
  - Комиссия «плавающая» (из цены газа и цены токена), её нужно брать свежей перед каждым выводом.
- **Минимум вывода — нет в документации.** В `user-withdraw-info` есть только максимумы: `spotMaxWithdrawAmount`,
  `chainLimit` и дневные лимиты в USD (`userDailyLimit`, `userRemainingDailyLimit`, `totalDailyLimit`, `dailyLimit` по
  монете).
- **Точность суммы вывода — нет в документации** для одиночного вывода. У `batch-transfer` написано «не больше
  8 знаков» ([C]), но это другой эндпоинт. Отдельно есть `decimals` токена.
- **Минимум депозита — нет в документации** (по уроку 21 это решение человека: `venue.deposit_min_absent_ok`).
- **Memo/tag:** для EVM неприменимо (счёт = адрес). Адрес депозита с memo есть только у SUI.
- **Подтверждения ввода — нет в документации** («once the transaction is confirmed on-chain»).

## 3. Вывод и депозит через API

**Вывод (спот, EVM).** В документации два варианта, какой основной — не сказано (вопрос Тиграну/в поддержку):
- `POST https://sapi.asterdex.com/api/v3/aster/user-withdraw` ([S] «Withdraw (USER_DATA)», вес 1);
- `POST https://chainapi.asterdex.com/aster-chain/v3/spot/user-withdraw` ([C], вес 5).

Как устроен вывод:
- Параметры: `chainId` (56), `asset`, `amount`, `fee`, `receiver`, `userNonce`, `userSignature`, необязательные
  `signatureChainId` и `signatureType` (`EOA`/`SafeWallet`).
- **Нужны две подписи:**
  1. обычная V3-подпись агента;
  2. отдельная EIP-712 подпись вывода. Домен `{name:"Aster", version:"1", chainId:56}`, тип `Action{type:"Withdraw",
     destination, "destination Chain":"BSC", token, amount, fee, nonce, "aster chain":"Mainnet"}`. В [D] сказано «by
     signing ... with your wallet». **По смыслу это ключ основного кошелька (`user`), а не агента** — прямо не сказано,
     проверить.
- Для нас это значит: боту нужен приватный ключ самого кошелька-счёта. Если счёт = DEX-кошелёк бота, ключ у бота уже
  есть (зашифрованный файл секретов).
- **Белый список адресов** как отдельной функции нет. Вместо него — «только на свой адрес» (см. п. 0). У агента с
  `canWithdraw=true` **обязателен `ipWhitelist`** ([F] «Register and Approve Agent»).
- Ответ: `withdrawId`, `hash`. Для Solana сказано, что `hash` — «not the transaction hash; just a unique identifier».
  Для EVM `hash` — «digest of the user's signature». **Это не хеш транзакции в сети.**
- **Своего ID для вывода (идемпотентности) нет.** Повтор после таймаута может дать двойной вывод. Есть ли защита по
  `userNonce` — нет в документации.
- **Статус и поиск вывода:** отдельного «вывод по id» нет. История: `POST /fapi/v3/aster/deposit-withdraw-history`
  ([D], V3) или `GET /aster-chain/v3/perp/deposit-withdraw-history?chainId=` ([C], только перп-счёт). Поля: `id`, `type`
  (DEPOSIT/WITHDRAW), `asset`, `amount`, `state`, `txHash`, `time`, `chainId`, `accountType` (spot/perp).
  - Совпадает ли `id` с `withdrawId` — нет в документации.
  - Параметров фильтра по id или времени нет.
- **Статусы:** [D] — `PROCESSING`, `SUCCESS`, `FAILED`. В примере [C] — `"COMPLETED"`. Противоречие, проверять живьём.
- Странная оговорка в [S]: «If the futures account balance is insufficient, funds will be transferred from the spot
  account to the perp account for withdrawal». Похоже, вывод идёт через перп-счёт. Как это влияет на баланс спота во
  время вывода — неясно, вопрос.

**Депозит (EVM):**
- Адреса депозита через API для BSC **нет**. Есть адрес контракта-хранилища BSC
  `0x128463A60784c4D3f46c23Af3f65Ed859Ba87974` ([C], [D] «Mainnet contract addresses») — по правилам проекта он идёт в
  настройку. `GET /aster-chain/v3/spot/user-deposit-address` — только для SUI.
- Зачисление на **спот — только при `depositFor(..., broker=1000)`**. Если просто перевести токен на хранилище, деньги
  зачисляются адресу отправителя, но на какой счёт (спот или перп) — нет в документации. Монета должна быть в списке
  deposit assets, иначе транзакция откатится с `CurrencyNotSupport`.
- История депозита — та же `deposit-withdraw-history`, там есть `txHash`. «Предварительно зачислено» (как MEXC
  PRE_SUCCESS) — **нет в документации**.
- Внутренний перевод с перп-счёта на спот — `POST /api/v3/asset/wallet/transfer` (`kindType FUTURE_SPOT`/`SPOT_FUTURE`,
  `clientTranId`) ([S] «Perp-spot transfer») или `/aster-chain/v3/spot/wallet/transfer` ([C]). При `broker=1000` он не
  нужен. Нужен как страховка, если депозит ушёл на перп (`venue.deposit_to_spot` — как у BingX).

## 4. Ордера

- `POST /api/v3/order` ([S] «Place order»), вес 1. `LIMIT` требует `timeInForce`, `quantity`, `price`.
- **IOC есть** (`timeInForce=IOC`, есть в `symbols[].timeInForce` примера). **FOK для спота не поддерживается:**
  `-1115 INVALID_TIF` ([S] «Enumeration definition → timeInForce»). Есть ещё `GTX` и `HIDDEN`.
- Свой ID: `newClientOrderId`, длина **не больше 36 символов** ([S] −4015). Алфавит не описан. Сказано: «Unless a
  previous order has already been filled, orders set with the same newClientOrderId will be rejected» — по уроку 6 ID
  всё равно новый на каждую попытку.
- Отмена и запрос — по `orderId` или `origClientOrderId` (`DELETE`/`GET /api/v3/order`).
  - Ордер `CANCELED`/`EXPIRED` без сделок старше 7 дней не возвращается.
  - `allOrders` и `userTrades` — окно 7 дней.
- **Итог ордера:** `avgPrice`, `executedQty`, `cumQuote`, `status`. **Комиссии в ответе ордера нет.** Комиссия и её
  монета — из `GET /api/v3/userTrades?orderId=` (`commission`, `commissionAsset`) или из потока `executionReport`
  (`n`, `N`).
  - `newOrderRespType` (ACK/RESULT/FULL) перечислен, но в параметрах ордера не описан, пример FULL не приведён.
- **Правила пары:** `PRICE_FILTER` (tickSize, minPrice, maxPrice; 0 = без ограничения), `LOT_SIZE`, `MARKET_LOT_SIZE`,
  `MIN_NOTIONAL`, `MAX_NOTIONAL`, `NOTIONAL`, `PERCENT_PRICE` (от индексной цены), `PERCENT_PRICE_BY_SIDE`,
  `MAX_NUM_ORDERS` ([S] «Filters»). Плюс `pricePrecision`, `quantityPrecision`.
  - В примере `MAX_NOTIONAL = 100` у BNBUSDT. **Есть максимум суммы ордера — учитывать в `order_rules::ready`.**
- **Флагов «покупка/продажа через API» у пары нет.** Статус пары — только `TRADING`.
- **Пробного ордера (`/order/test`) в спот API нет.** `venue.order_test_first` = да для Aster — ошибка настройки.
- **Пары `TEST*` — только для внутреннего тестирования Aster, торговать нельзя** ([S] «Attention»). Отсеивать
  (настройкой — список префиксов).
- `POST /api/v3/noop` — «cancel previously sent transactions that are still in the queue and have not completed the on
  chain operation». Значит, ордер проходит через очередь Aster L1, и исход при таймауте может быть неизвестен
  (OutcomeUnknown, решение 64). Поиск по `origClientOrderId` есть.

## 5. Стакан

- REST: `GET /api/v3/depth?symbol=&limit=` (5/10/20/50/100/500/1000; вес 2/2/2/2/5/10/20) → `lastUpdateId`, `E`, `T`,
  `bids`, `asks` ([S] «Depth information»).
- WS, частичный: `<symbol>@depth<5|10|20>` или `...@100ms` (1000 мс / 100 мс). Пример payload — `depthUpdate` с `U/u/pu`
  и полями `bids/asks` (как у фьючерсов Binance). Снимок это или изменения — из примера не ясно, проверить живьём
  ([S] «Limited depth information»).
- WS, изменения: `<symbol>@depth` или `@depth@100ms`, поля `U`, `u`, `pu`, `b`, `a` ([S] «Incremental depth
  information»).
  - Порядок сборки — как у спота Binance: буфер → REST-снимок 1000 → отбросить `u <= lastUpdateId` → первое событие
    `U <= lastUpdateId+1 <= u`.
  - **Дальше `U == предыдущий u + 1`, иначе пересинхронизация** ([S] «How to correctly maintain a local copy»).
  - При этом в payload есть `pu` (фьючерсная проверка `pu == предыдущий u`). Какая проверка верна — проверить на живых
    кадрах.
- Лимиты WS ([S] «WebSocket connection limits»): **до 200 потоков на соединение; не больше 5 сообщений в секунду**
  (PING, PONG, SUBSCRIBE считаются). Превышение → разрыв, при повторах — блок IP. Лимита соединений с IP в документации
  нет.
- Пинг: сервер шлёт ping каждые 5 минут, pong нужен в течение 15 минут. Можно слать непарные pong или JSON
  `{"method":"PONG"}` ([S] «WebSocket market data feed»). Не путать с лимитом 5 сообщений/с.
- Символы в именах потоков — строчными буквами.

## 6. Комиссии

- По паре через API: `GET /api/v3/commissionRate?symbol=` → `makerCommissionRate`, `takerCommissionRate` (вес 20,
  [S] «Get symbol fees»). В разделе стоит «Weight 20» без пометки USER_DATA — подписан ли запрос, проверить. В счёте
  есть `feeTier`.
- Публичная ставка спота: maker 0,005 %, taker 0,04 %; уровни VIP — по 14-дневному объёму ([W2]).
- **Скидка своей монетой:** ASTER, −5 %. Включается **только в интерфейсе сайта** переключателем «Pay fee in ASTER»;
  работает, пока на споте есть ASTER ([W2]). **В API нет ни переключателя, ни чтения его состояния.** Факт — из
  `commissionAsset` / `N`. По уроку 23: считаем по обычной ставке (худший случай), монета комиссии — по факту.

## 7. Лимиты запросов

- Лимиты отдаются в `exchangeInfo.rateLimits`.
  - В примере ответа: `REQUEST_WEIGHT` 6000/мин; `ORDERS` 6000/мин и 300/10 с.
  - В разделе «Enumeration» — `REQUEST_WEIGHT` 1200/мин и `ORDERS` 100/мин.
  - Противоречие. Брать из живого `rateLimits` ([S] «Trading specification information», «Rate limit type»).
- Вес — **на IP** («Access restrictions are based on IP, not API Key»). Заголовок `X-MBX-USED-WEIGHT-(n)(x)`. Ордера —
  на счёт, заголовок `X-MBX-ORDER-COUNT-*`.
- 429 → остановиться, `Retry-After` в секундах. Продолжение → 418, бан от 2 минут до 3 суток. **429 по ордерам приходит
  без `Retry-After`** — пауза по `venue.rate_limit_pause_ms` ([S] «IP access limits», «Order rate limits»).
- −1003 TOO_MANY_REQUESTS, −1015 TOO_MANY_ORDERS.
- Лимиты `bapi` (`www.asterdex.com`) и `chainapi` (кроме весов эндпоинтов) — нет в документации.

## 8. Приватный WS (поток аккаунта)

- Есть. `POST /api/v3/listenKey` (USER_STREAM, т. е. с V3-подписью) → `listenKey`, срок 60 минут. `PUT` продлевает
  (совет — раз в 30 минут), `DELETE` закрывает ([S] «Listen Key (spot account)»). Подключение —
  `wss://sstream.asterdex.com/ws/<listenKey>`.
- События:
  - `outboundAccountPosition` (`B[] {a,f,l}`, `m` — причина, например `WITHDRAW`);
  - `executionReport` (`c`, `X`, `x`, `z`, `ap`, `n`, `N`, `Z`).
- Хеша транзакции в событии баланса нет → событие только будит проверку, зачисление закрывается по истории
  (как MEXC).
- Перечень значений `m` и событие «listenKey истёк» для спота — нет в документации.

## 9. Счета (панель «Балансы»)

| Счёт | Эндпоинт | Источник |
|---|---|---|
| Спот | `GET /api/v3/account` → `balances[] {asset, free, locked}`, флаги `canTrade/canDeposit/canWithdraw`, `feeTier` | [S] «Account information» |
| Перп (фьючерсы) | `GET /fapi/v3/balance`, `GET /fapi/v3/account` | [F] «Futures Account Balance v3», «Account Information v3» |
| Доступно к выводу (спот/перп по цепи) | `user-withdraw-info` | [D], [C] |
| Стейкинг ASTER | `/aster-chain/v3/staking/...` («Get My Staking», «Get Staking Account Status») | [C] |
| Фонд/funding, Earn | **нет таких счетов в документации** | — |

Баланс одной монеты отдельным запросом — нет (решение 113: фильтр по общему списку). Если `GET /api/v3/account`
вызвать до первого депозита, будет −5050.

## 10. Права ключа

- Ключ = **API-кошелёк (агент)**: отдельный EOA, его ключ хранится у бота. Его регистрирует и подписывает основной
  кошелёк: `POST /fapi/v3/registerAndApproveAgent` или `approveAgent` ([F] «Register and Approve Agent»).
  - Права: `canSpotTrade`, `canPerpTrade`, `canWithdraw`; срок `expired` (агент истекает сам); `ipWhitelist`
    (обязателен при `canWithdraw`).
  - Для нас: `canSpotTrade=true`, `canWithdraw=true`, `canPerpTrade=false` (урок 19). Права перевода спот↔перп
    отдельно не описаны (`TRADE`).
- **Проверка прав через API:** `GET /fapi/v3/agent` — только в демо [A] (формат ответа не описан, в справочнике
  эндпоинта нет). Надёжно описаны только флаги счёта `canTrade/canWithdraw/canDeposit` из `/api/v3/account` — это флаги
  счёта, а не агента.
  - Для «Проверить подключение»: подписанный `GET /api/v3/account` (ключ живой, депозит сделан — нет −5050) и, если
    подтвердится живьём, `GET /fapi/v3/agent`: права, IP, `expired`.
  - Срок агента — уведомление заранее (как срок ключа MEXC).

## 11. Особенности, которые ломают наш поток — по 30 урокам

| # | Урок | Aster | Как закрыть |
|---|---|---|---|
| 1 | Только по документации | Часть нужного — в `demo/` и `bapi` сайта, не в справочнике | Ссылки на [D]/[A] в комментариях. Каждый неописанный пункт — вопрос в `docs/TASKS.md` |
| 2 | Противоречия — по живому ответу | Много: окно nonce (60/30/10 с), лимиты (1200 против 6000), статусы истории (SUCCESS против COMPLETED), `pu` против `U=u+1`, вывод «только на свой адрес» против гида [W1] | Записать живые ответы с сервера бота до кода |
| 3 | Тесты на записанных данных | Живого ответа нет (сеть закрыта) | Запись публичных ответов на сервере; приватные — `#[ignore]` |
| 4 | Одна функция правил ордера | Фильтры как у Binance + **MAX_NOTIONAL** + PERCENT_PRICE от индекса | `order_rules::ready`, максимум суммы — обязательно |
| 5 | IOC | Есть. FOK — нет (−1115) | `LIMIT` + `IOC` |
| 6 | Новый ID на попытку | `newClientOrderId` ≤ 36 | Префикс + счётчик, длина — настройка |
| 7 | Итог ордера | Комиссии нет в ответе ордера; окна истории — 7 дней | Комиссия — из `userTrades` / `executionReport`; срок 7 дней — настройка, проверка до ордера |
| 8 | Сопоставление по адресу контракта | Адрес есть только в `bapi` deposit/withdraw assets, сеть — число `56` | `venue.bsc_network` = 56. Сопоставление `name` ↔ `baseAsset` — проверить. Пары `TEST*` отсеивать |
| 9 | Большой справочник | Размер неизвестен (спот Aster, по всей видимости, небольшой — проверить) | Как MEXC: `spawn_blocking`, `Arc` |
| 10 | Ввод/вывод, деньги не на споте | Депозит = вызов `depositFor(broker=1000)`; иначе может уйти на перп | **Нужна новая операция DEX-кошелька «вклад в контракт»**; страховка — перевод FUTURE_SPOT |
| 11 | Комиссия по паре | `commissionRate` | Фоновый круг |
| 12 | Стакан WS, лимиты | 200 потоков на соединение, **5 сообщений/с всего**, ping 5 мин / pong 15 мин | `venue.ws_symbols_per_conn` ≤ 200, `venue.ws_subs_per_sec` ≤ 5 вместе с pong |
| 13 | Меньше выделений памяти | JSON как у Binance | Как bingx/mexc |
| 14 | Пересподписка без перезапуска кругов | — | Как у MEXC (решение 131) |
| 15 | REST-лимиты, Retry-After | Вес на IP; 429 по ордерам без Retry-After | Пауза по Retry-After или `venue.rate_limit_pause_ms` |
| 16 | Приватный WS | listenKey 60 мин | Продление по настройке |
| 17 | Пять частей фичи | + новые секреты: ключ агента, адрес `user`, ключ основного кошелька для подписи вывода | Экран «Площадки» + «Проверить подключение» |
| 18 | Секреты | **Ключ основного кошелька нужен для каждого вывода** | Только из файла секретов. Если счёт = DEX-кошелёк — тот же ключ, без дубля |
| 19 | Права ключа | Агент: spot + withdraw, perp = false, IP обязателен | Описать в README адаптера |
| 20 | Адаптер не меняет ядро/шлюз | **Ввод на Aster — вызов контракта из кошелька, а не перевод на адрес** | Нужно решение Тиграна: `deposit_address` = адрес хранилища не годится (обычный перевод зачислит отправителю, счёт не указан) |
| 21 | Минимум депозита | Нет в API | `venue.deposit_min_absent_ok` — решение человека |
| 22 | Удержание вывода | Статуса PRE_SUCCESS нет в документации | Неприменимо, пока не появится в живом ответе |
| 23 | Скидка своей монетой | ASTER −5 %, только переключатель на сайте, в API нет | Расчёт по обычной ставке, факт — `commissionAsset` |
| 24 | Балансы всех счетов | Спот, перп, стейкинг; funding/earn нет | `account_balances` |
| 25 | Кэш адреса депозита | Адреса нет, есть хранилище (одно на цепь) | Настройка + проверка, что монета есть в deposit assets |
| 26 | Повтор круга комиссий | — | `venue.fee_retry_s` |
| 27 | «Счёт не открыт» | **−5050 DEPOSIT_REQUIRED** до первого депозита | Показать точную причину, не ошибка связи |
| 28 | gzip, IPv4/IPv6 | Нет в документации | `venue.api_ip` как у Gate, по живому |
| 29 | Своя запись ключа | `GET /fapi/v3/agent` — только в демо | Проверить живьём |
| 30 | IOC живым ордером | IOC в `timeInForce` пары есть | Проверить живым IOC-ордером на маленькой сумме (пробного ордера нет) |

Ещё риски, которых нет в чек-листе:
- **Вывод без своего ID (нет идемпотентности)** и без поиска по id. При таймауте неизвестно, был ли вывод.
- **Ответ вывода не даёт хеш транзакции.** `txHash` появляется только в истории. Нужно сопоставлять запись истории с
  выводом по времени и сумме, пока не подтверждено, что `id == withdrawId`.
- **Дневной лимит вывода в USD** (`userRemainingDailyLimit`, `dailyLimit`) — проверять в предпроверке до покупки.
- **Ордер проходит через очередь L1** (Noop) — задержка и неизвестный исход вероятнее, чем на CEX.
- **Направление CEX ↔ Aster невозможно** (п. 0) — Aster будет только площадкой «биржа ↔ DEX».
- Ликвидность и число спотовых пар Aster неизвестны (живого `exchangeInfo` нет). Возможно, монет, которых нет на BingX
  или MEXC, мало.

## 12. Вердикт

**НЕ ПОДКЛЮЧАТЬ сейчас по шаблону CEX-адаптера.** Подключать **только после решения Тиграна** по модели ввода/вывода.
Сложность — **высокая**.

Почему:
1. Спот есть, торговое API — Binance-подобное (как MEXC): IOC, фильтры, `@depth`, listenKey. Торговая часть сама по себе
   средней сложности (подпись EIP-712 через alloy).
2. Ввод/вывод — не биржевые, а ончейн-модель кошелька:
   - адреса депозита нет, есть вызов контракта `depositFor(broker=1000)`;
   - вывод только на свой адрес, с отдельной EIP-712 подписью ключом основного кошелька;
   - комиссия вывода «плавающая», её передаёт клиент;
   - нет своего ID вывода и поиска по нему.
3. Из-за п. 2 **CEX ↔ Aster невозможно**. Aster ↔ DEX возможно, только если кошелёк-счёт Aster = DEX-кошелёк из
   «Маппинга». Для ввода нужна новая операция кошелька «вклад в контракт» — это изменение DEX-адаптера и, скорее всего,
   контракта шлюза (урок 20), то есть нужно решение.
4. Ключевые данные (адрес контракта, комиссия вывода, лимиты вывода) — частично только в `demo/` и `bapi` сайта, не в
   справочнике API. Противоречий много, живьём ничего не проверено.

На кого похож: торговля — MEXC/Binance (почти один в один по полям). Ввод/вывод — ни на одну из подключённых бирж не
похож, ближе к нашему DEX-кошельку (ончейн-вызов контракта и подписи EIP-712).

### Вопросы Тиграну (до решения)
1. Нужен ли Aster в режиме только «Aster ↔ DEX»: счёт Aster = DEX-кошелёк, ввод вызовом `depositFor`? Готов ли ты
   разрешить новую операцию кошелька «вклад в контракт» (изменение DEX-адаптера и контракта шлюза)?
2. Ключ основного кошелька для подписи каждого вывода — то же, что ключ DEX-кошелька. Это допустимо?
3. Какой эндпоинт вывода основной — `sapi /api/v3/aster/user-withdraw` или `chainapi /aster-chain/v3/spot/user-withdraw`?
   Для поддержки Aster: совпадает ли `id` истории с `withdrawId`, какие точные статусы, можно ли выводить на чужой адрес
   (гид [W1] против API).
4. Записать на сервере бота: `exchangeInfo`, deposit/withdraw assets (chainId 56, spot), `estimateFee`, `depth`, кадры
   `@depth` и `@depth5@100ms` — чтобы оценить число пар и пересечение с BSC-монетами до любой разработки.
