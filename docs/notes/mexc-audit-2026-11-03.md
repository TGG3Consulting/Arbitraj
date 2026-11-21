# MEXC — сравнительный аудит адаптера против официальной документации

Claude, 03.11.2026 (UTC 02.11.2026). Код **не менялся** — только отчёт. Решения по исправлениям принимает Тигран.

## Источники и версии

| Метка | Источник | Версия, по которой сверял |
|---|---|---|
| [L] | Живые страницы `https://www.mexc.com/api-docs/spot-v3/...` (Docusaurus 3.8.1, «version: current») | Прочитаны 02.11.2026 (UTC). Последняя запись Change Log — **2026-10-23** (код 30035) |
| [G] | `github.com/mexcdevelop/apidocs`, ветка `spot_v3_en` | Коммит `349c691` от 2025-11-20 (старше [L]; при расхождении верим [L]) |
| [P] | `github.com/mexcdevelop/websocket-proto` | Коммит `0c9c4f3` от 2026-09-09 (последний) |
| [R] | Живые записи `tests/data/mexc_2026-11-02/exchange_info.json`, DECISIONS 107–115 | 02.11.2026 |

Оговорка о точности цитат. Страницы [L] читались через WebFetch — он отдаёт дословно только короткие куски (до 125 знаков), длинные
таблицы пересказывает. Ниже «в кавычках» — дословно; без кавычек и с пометкой (пересказ) — пересказ страницы. Где в документации
чего-то нет — так и написано: «в документации нет». Из рабочей среды Claude сам API MEXC (api.mexc.com) недоступен (прокси 403) —
живые ответы, кроме [R], не проверялись.

Change Log [L] с прошлого разбора (что касается нас): 2026-10-23 — новый код **30035** «You have triggered the order limit, please contact us by
submitting a ticket.»; 2026-08-24 — задокументирован `GET /api/v3/apiKeyInfo` (SPOT_ACCOUNT_READ, вес IP 1); 2026-07-21 — код 157005;
2026-05-29 — поле `depositPreConfirms` в getall; 2026-05-22 — `DELETE /api/v3/order/all`; 2026-05-21 — в частичном стакане задокументированы
`version`, `lastOrderCreateTime`; 2026-05-03 — «IP и UID — независимые корзины: IP 300 / 10 с, UID 500 / 10 с»; 2026-04-25 — myTrades:
`orderListId`, `isBuyer`, `isMaker`; 2026-03-11 — постановка и отмена ордеров — общий лимит UID 12 запросов/с; 2026-02-11 — код 11444;
2025-12-05 — `GET userDataStream`: `total`, `available`. **Новых имён или версий каналов WebSocket нет** — `spot@public.limit.depth.v3.api.pb`
и `spot@private.account.v3.api.pb` действуют; `.proto` [P] совпадают с `proto.rs` полем в поле.

---

## Итог: список проблем по критичности

🔴 — ломает торговлю или деньги в обычной работе; 🟠 — ломает в редких случаях; 🟡 — расхождение без вреда сейчас.

### 🔴 Критичные

**🔴1. Комиссия ордера считается по первым 10 сделкам (myTrades без `limit`).**
- Дока [L] https://www.mexc.com/api-docs/spot-v3/spot-account-trade/account-trade-list: параметр `limit` int, NO — «Default 10; max 1000» (пересказ таблицы: по умолчанию 10, максимум 1000); `orderId` — необязательный фильтр.
- У нас `order_state.rs:121-125` — запрос `GET /api/v3/myTrades` только с `symbol` и `orderId`, без `limit`; `order_state.rs:130-147` — сумма `commission` по пришедшим.
- Риск: ордер IOC/LIMIT по худшей цене проходит по 20 уровням и бьётся об десятки встречных ордеров — сделок у ордера больше 10. Комиссия занижается, учёт сделки (`core/fills.rs:98`) думает, что монет больше, чем есть: продажа или вывод «на всё» получают 10101 / 30005 «Oversold» → застрявшая монета, ложное расхождение сверки.
- Исправить: передавать `limit` (настройкой, предел 1000 из доки); сверять сумму `qty` сделок с `executedQty` ордера — не сошлось (сделки ещё не видны) — повторить чтение, а не отдавать занижённую комиссию.

**🔴2. Первый же сбой запроса статуса ордера — «итог неизвестен» без отмены: ордер LIMIT остаётся в стакане.**
- Дока [L] …/query-order: `GET /api/v3/order`, вес IP 2, нужен **SPOT_DEAL_READ**. Отмена [L] …/cancel-order: «Either `orderId` or `origClientOrderId` must be sent.»
- У нас `order_state.rs:40` — `query(...).await?` внутри цикла ожидания: любая ошибка запроса (сеть, 429, 5XX, −2011 «Unknown order sent» сразу после постановки, 700007 без права) сразу выходит из `settle` через `unknown()` (`order_state.rs:152-157`), **до** отмены на `order_state.rs:53`.
- Риск: в живом `orderTypes` нет IMMEDIATE_OR_CANCEL ([R], DECISIONS 107), значит рабочий тип — `venue.order_type = LIMIT`. Ордер LIMIT по худшей цене висит в стакане, сделка в Halted, деньги исполняются потом без учёта. Нарушается стартовое правило Тиграна «не исполнился — отменяем».
- Исправить: ошибка запроса в цикле — не выход, а повтор до `venue.order_final_wait_ms`; по сроку — всегда DELETE (по `orderId`), затем чтение итога; и только если итог не прочитан после отмены — OutcomeUnknown.

**🔴3. HTTP 429 с телом MEXC не становится RateLimited; заголовок Retry-After не читается.**
- Дока [L] https://www.mexc.com/api-docs/spot-v3/introduction: «HTTP 429 indicates the request rate limit is being exceeded»; после 429 надо остановить запросы, дальше — бан IP «from a minimum of 2 minutes to a maximum of 3 days»; «Responses with 418 or 429 include a `Retry-After` header indicating the wait time in seconds». Таблица кодов [L] …/error-code: `429 — "Too Many Requests"`, `403 — "Access Denied"`, `401 — "No authority"`.
- У нас `rest.rs:160-163` — если в теле есть `code` и `msg`, ошибка берётся из `errors::from_code` **независимо от HTTP-статуса**; в `errors.rs:31-49` кода 429 нет → для ордера `OrderRejected`, для остального `BadResponse`. До `errors::from_http` (`errors.rs:90-93`, там RateLimited) дело доходит, только если тело не JSON. `gateway/http.rs` отдаёт только статус и текст — заголовки (Retry-After) теряются.
- Риск: правило шлюза на `RateLimited` (`core/rules_apply.rs:37`) не срабатывает, бот продолжает слать запросы → бан IP на минуты–дни, в том числе посреди сделки (продать монету на MEXC нечем).
- Исправить: HTTP 418/429 (и code 429 в теле) → всегда RateLimited, пауза площадки не меньше Retry-After; 403 → «WAF», как в `from_http`.

### 🟠 Ломает в редких случаях

**🟠1. Бюджет веса IP 300 / 10 с съедает фоновое обновление комиссий.**
- Дока [L] …/query-symbol-commission: `tradeFee` — «Weight: 20 (IP)»; введение [L]: IP-корзина «300 weight per 10 seconds», UID — 500, «independent».
- У нас `fees.rs:10-11` — в комментарии «вес 20 из 500 на UID» (неверно: IP); README — «`venue.fee_requests_per_sec` = 1». При 1 запросе/с это 200 из 300 веса IP; плюс опрос ордера (`order_state.rs:40`, вес 2 каждые `venue.order_poll_ms`), история депозитов (`transfers.rs:131-137`, вес 10 каждые `venue.status_poll_ms`), getall (10), exchangeInfo (25), account (10), myTrades (10).
- Риск: во время сделки (частый опрос ордера + статусы переводов) корзина переполняется → 429 (см. 🔴3).
- Исправить: исправить комментарий; в интерфейсе — предел `venue.fee_requests_per_sec` по весу; либо реже (`venue.fee_cache_s` большой), либо брать ставки разом (в [L] у `exchangeInfo` есть `makerCommission`/`takerCommission`, но это ставки платформы, не ключа — решать Тиграну).

**🟠2. «Проверить подключение» не требует право чтения ордеров SPOT_DEAL_READ.**
- Дока [L] …/query-order: «Required permission: SPOT_DEAL_READ» (пересказ). [L] …/query-api-key-info: `permissions` — список через запятую, значения: SPOT_ACCOUNT_READ, SPOT_ACCOUNT_WRITE, SPOT_DEAL_READ, SPOT_DEAL_WRITE, CONTRACT_*, SPOT_TRANSFER_READ, SPOT_TRANSFER_WRITE (пересказ).
- У нас `account.rs:27-32` — REQUIRED = SPOT_ACCOUNT_READ, SPOT_DEAL_WRITE, SPOT_WITHDRAW_READ, SPOT_WITHDRAW_WRITE (нет SPOT_DEAL_READ).
- Риск: ключ без SPOT_DEAL_READ проходит проверку «Подключено», ордер ставится, а статус не читается (700007) → 🔴2 (ордер висит, сделка Halted).
- Отдельно: в перечне значений `permissions` на странице apiKeyInfo нет SPOT_WITHDRAW_READ / SPOT_WITHDRAW_WRITE, хотя эндпоинты кошелька требуют именно их — противоречие в доке; если живой ответ называет права иначе, проверка всегда «нет прав». Сверить на живом ответе apiKeyInfo (он уже пишется в сырой журнал).

**🟠3. Ответ на постановку ордера / вывод не разобран → BadResponse вместо «итог неизвестен».**
- Дока [L] …/new-order: ответ `{"symbol","orderId","orderListId","price","origQty","type","side","stpMode","transactTime"}`; …/withdrawnew: ответ `{"id":"7213fea8e94b4a5593d507237e5a555b"}`.
- У нас `trade.rs:147` — `num::text(&p, "orderId")?` и `transfers.rs:75` — `num::text(&data, "id")?`: ошибка разбора уходит наружу как `BadResponse`, мимо `errors::unsure`.
- Риск: MEXC ордер/вывод принял (HTTP 200), а ядро видит обычный сбой; новая попытка идёт с новым newClientOrderId (решение 114) → двойной ордер.
- Исправить: при успешном HTTP без `orderId`/`id` — OutcomeUnknown (для ордера — поиск по origClientOrderId).

**🟠4. Итог постановки неизвестен и ордер не найден — нет отмены по своему ID.**
- Дока [L] …/cancel-order: `origClientOrderId` — допустимый ключ отмены. Query order [L]: поиск по `origClientOrderId`.
- У нас `trade.rs:149-154` → `order_state.rs:66-71`: один запрос по `origClientOrderId`; не найден (ордер ещё не проиндексирован) — сразу OutcomeUnknown; отмены нет.
- Риск: с `LIMIT` ордер может появиться и висеть.
- Исправить: при неизвестном итоге — DELETE по `origClientOrderId`, затем чтение по нему же (несколько попыток в пределах `venue.order_final_wait_ms`).

**🟠5. Вывод: при неизвестном итоге нет поиска по `withdrawOrderId`; повтор вывода той же сделки — тот же `withdrawOrderId`.**
- Дока [L] …/withdraw-history-supporting-network: в ответе есть поле `withdrawOrderId`. [L] …/withdrawnew: `withdrawOrderId` string NO — **правил длины, символов и уникальности в документации нет**.
- У нас `transfers.rs:38-43` — `withdrawOrderId` = буквы и цифры номера сделки (одинаков у всех выводов сделки, в том числе повторного и возврата USDT, если он под тем же номером); `transfers.rs:93-96` — статус ищется только по `id`.
- Риск: (а) тот же класс сбоя, что «duplicate client order id» у ордеров (решение 114) — если MEXC требует уникальность, второй вывод сделки отклоняется; (б) OutcomeUnknown у вывода не довести — сделка ждёт ручной проверки.
- Исправить: метка попытки в `withdrawOrderId` (как в решении 114); при OutcomeUnknown — искать в истории по `withdrawOrderId`. Уникальность — спросить/проверить на живом выводе.

**🟠6. Хеш депозита сравнивается как строка целиком; формат `txId` в доке не задан.**
- Дока [L] …/deposit-historysupporting-network: пример `"txId": "01391d1c1397ef0a3cbb3c7f99a90846f7c8c2a8dddcdcf84f46b530dede203e1bc804"` — без `0x`; формат не описан. История выводов [L]: `transHash` с `0x`.
- У нас `transfers.rs:139-142`, `178-181` — `eq_ignore_ascii_case` полной строки.
- Риск: если MEXC отдаёт хеш BSC без `0x` или с суффиксом (у других бирж бывает «хеш:индекс») — депозит никогда не «виден» → сделка ждёт до таймаута.
- Исправить: сравнивать после нормализации (без `0x`, до `:`); проверить на первой живой записи hisrec.

**🟠7. Шаг суммы вывода `withdrawIntegerMultiple` не используется.**
- Дока [L] …/query-the-currency-information: `withdrawIntegerMultiple` — шаг суммы вывода (пересказ: «Step size for withdrawal amounts»); код ошибки [L] 10096 «amount decimal places is too long».
- У нас `wallet.rs:14-15` — «смысл не ясен, не используется»; точность вывода — `baseAssetPrecision` пары (`wallet.rs:160`, `symbols.rs:143-152`, решение 107 г).
- Риск: у монеты, где шаг вывода крупнее точности пары, вывод отклоняется (10096), монета застревает на MEXC.
- Исправить: на живом getall посмотреть значения поля у BSC-сетей; если это шаг — учитывать в точности вывода. Решение Тиграна.

**🟠8. Свежесть стакана — по соединению, а не по паре.**
- Дока [L] …/partial-book-depth-streams: частота отправки «не указана» (в документации нет).
- У нас `book.rs:70-77` (решение 109) — котировка свежая, пока по соединению идёт любой кадр, включая PONG (`ws_conn.rs:100-101`); у пары время последнего кадра не хранится.
- Риск: у пары, по которой MEXC перестал присылать стакан (пауза торгов, сбой подписки после переподключения на стороне MEXC), стакан в памяти «свежий» бесконечно — сделка по старой цене.
- Исправить: время последнего кадра стакана по паре + свой порог (настройка); решение 109 сохраняет экономию CPU, т.к. время пишется и при пропуске одинакового снимка.

**🟠9. Метка времени — по своим часам, без сверки со временем сервера.**
- Дока [L] introduction: «if (timestamp < (serverTime + 1000) && (serverTime - timestamp) <= recvWindow)».
- У нас `rest.rs:56-59` — `chrono::Utc::now()`; `GET /api/v3/time` не используется. 700003 «Timestamp for this request is outside of the recvWindow» переводится в `InvalidCredentials` (`errors.rs:34`).
- Риск: часы сервера убежали вперёд больше чем на 1 с — все подписанные запросы, включая ордера и выводы, отклоняются с текстом «неверный ключ».
- Исправить: 700003 и 10073 — отдельная причина «часы»; при желании — поправка по `serverTime`.

**🟠10. Ключи listenKey: новый при каждом переподключении.**
- Дока [L] …/websocket-user-data-streams/: «Each UID can apply for a maximum of 60 listen keys (excluding invalid listen keys)», «Each listen key maximum support 5 websocket connection».
- У нас `user_stream.rs:56` — `create` на каждом круге; `user_key.rs:52-55` — закрытие без проверки (ошибка проглатывается).
- Риск: частые обрывы при недоступном закрытии (сеть) — до 60 живых ключей за час → `POST userDataStream` отказывает, поток баланса стоит.
- Исправить: при обрыве WebSocket, если ключ жив (продление прошло), переподключаться с тем же ключом; новый — только при отказе продления.

### 🟡 Расхождения без вреда сейчас

1. **recvWindow = 60000.** [L] код 700005 «recvWindow must less than 60000»; new-order: `recvWindow` «Max 60000». У нас `rest.rs:26,48` — допускается ровно 60000. Сделать `>=`.
2. **Коды ошибок без своего перевода** (`errors.rs:31-49`): 30035 (новый, 2026-10-23, блок ордеров на аккаунте — стоит паузы площадки, а не одной отказанной сделки), 403 «Access Denied» (в теле), 10073 «invalid Request-Time», 700004/700005/700008, 30000/30001/30016/30018/30019/30041, 22222, 26, 60005 «your account is abnormal». Все попадают в OrderRejected / WithdrawRejected / BadResponse с текстом MEXC — деньги не двигаются, но правило шлюза не различает «пара закрыта» и «аккаунт заблокирован».
3. **11444 «This feature is temporarily unavailable…»** у ордера/вывода переводится в Timeout → OutcomeUnknown (`errors.rs:40`, `57-60`) — это однозначный отказ, сделка зря уходит в ручную проверку.
4. **Комментарий веса tradeFee** (`fees.rs:10-11`, README:39) — «UID»; в [L] — «IP».
5. **apiKeyInfo**: в таблице поле `apikey`, в примере `accessKey` (противоречие доки, нас не касается — поле не читаем). Ответ apiKeyInfo пишется в сырой журнал (`rest.rs:150-153`) вместе с `accessKey` и `ipWhiteList`; ошибка reqwest может нести URL с `accessKey=` в тексте проверки.
6. **Баланс**: [L] account-information добавила поле `available` («coin amount available to use»); мы берём `free` (`account.rs:43-44`). Сверить на живом ответе, совпадают ли.
7. **`orderTypes` и PERCENT_PRICE_BY_SIDE не проверяются до отправки** (`symbols.rs:195-226`, `trade.rs:19-20`). [L]: покупка `orderPrice <= lastPrice * (1 + bidMultiplierUp)`, продажа `orderPrice >= lastPrice * (1 - askMultiplierDown)`. Отказ ловит пробный ордер или MEXC (30041, 33333) — денег не стоит, но лишний запрос и отказ сделки вместо отсева.
8. **Пробный ордер перед каждым** (`trade.rs:129-144`) — плюс один подписанный запрос (вес 1) на пути сделки; любая его ошибка (в том числе 429/сеть) становится «пробный ордер отклонён» (OrderRejected) — причина «лимит» теряется.
9. **`origQty` нет — берётся `req.qty`** (`order_state.rs:82-84`): а отправляли округлённое `qty` (`trade.rs:50`). Если MEXC не пришлёт ни `origQty`, ни `Qty` (в [L] query-order — поле `Qty`, в cancel — `origQty`), полностью исполненный ордер будет «частичным». Лучше сравнивать с отправленным количеством или брать `status`.
10. **Частичный стакан**: в [L] описание `price` — «Price level where the change occurred», в примере `"asks": []`. Мы считаем каждый кадр полным снимком (`depth.rs:89-106`) — так и есть на живых кадрах (решение 109); `version` не проверяется на порядок кадров.
11. **`venue.ws_subs_per_sec` не ограничен** пределом доки (в [L] введение: соединение — 100 запросов/с, частые переподключения — блок IP); `venue.ws_ping_ms` не сверяется с `venue.ws_silence_ms` и минутой MEXC («with a successful subscription but no data flow, the server disconnects after one minute») — для потока аккаунта, где данных почти нет, PING должен быть заметно меньше 60 с (`user_stream.rs:113-129`, `ws.rs:108-120`).
12. **`selfSymbols` в справочнике пар** (`symbols.rs:82`) — подписанный: пока секреты заперты, `refresh` падает, и подписки на стаканы MEXC нет совсем (стаканы публичные).
13. **`coin_networks_of` / `withdraw_cost` / `deposit_address`** (`wallet.rs:224`, `263`) — весь getall (~5.6 МБ, вес 10) на каждый вызов; [L]: «Request parameters: None» — фильтра по монете у MEXC нет (решение 113). На пути сделки — задержка.
14. **`symbols.ensure_fresh` в `trade`** (`mod.rs:130`) при возрасте ≥ `venue.symbols_refresh_s` перечитывает весь exchangeInfo (вес 25) на пути сделки; фоновый `pairs_watch` обновляет с тем же периодом — совпадение возможно.
15. **`precision_of` для котируемой монеты** (`symbols.rs:148-151`) — `symbol.ends_with(ticker)`: у монеты без своей пары, чей тикер — хвост «USDT» (T, DT, SDT), возьмётся чужая точность.
16. **Ответ с `code` ≠ 0/200, но без `msg`** (`rest.rs:164`) считается успехом. В доке формат ошибки один — `{code, msg}`, но схема ответа об ошибке в документации нет (прошлый разбор, раздел 5).
17. **Генерация адреса депозита** `POST /api/v3/capital/deposit/address` ([L] …/generate-deposit-address-supporting-network: coin YES, network YES, SPOT_WITHDRAW_WRITE, вес 1) не используется — при отсутствии адреса бот просит создать его в кабинете (`wallet.rs:282-291`). Осознанно.
18. **История выводов / депозитов** — без `startTime`: [L] по умолчанию «last 7 days», `limit` default 1000 (`transfers.rs:84-92`, `130-137`). Перевод старше 7 дней не найдётся — для нашего цикла неважно.
19. **`account` — «rate limit: 2 times/s»** ([G] 2025-11-20; в [L] не указано). Своего предела у `balances()` нет.

---

## Таблицы сравнения

Формат: эндпоинт/канал | офдока | у нас (файл:строка) | совпадает? | риск | что исправить.

### 1. Подпись, заголовки, общие правила REST

| Что | Офдока [L] introduction | У нас | Итог | Риск | Исправить |
|---|---|---|---|---|---|
| Адрес | «Use the following base URL…» `https://api.mexc.com` | `venue.rest_url` + резерв `venue.rest_fallback_url` (`ctx.rs:95-104`) | ✅ (настройка) | — | — |
| Алгоритм | HMAC SHA256, ключ — API Secret | `rest.rs:61-69` HMAC-SHA256 по секрету | ✅ | — | — |
| Что подписывается | `totalParams = queryString + requestBody`, без `&` между ними; `signature` не входит | Все параметры в строке запроса, тела нет → подписывается строка запроса (`rest.rs:60`, `70`) | ✅ | — | — |
| Порядок параметров | «Do not reorder parameters after generating the signature» (пересказ) | Порядок `Vec` = порядок подписи = порядок отправки (`rest.rs:28-29`, `77-90`) | ✅ | — | — |
| Кодирование | «URL-encode special characters such as commas before signing… encoding must use uppercase only» | `encode` — всё, кроме `A-Za-z0-9-_.~`, в `%XX` верхним регистром (`rest.rs:93-103`) | ✅ | — | — |
| Регистр hex подписи | «lowercase hexadecimal only» | `hex::encode` — нижний регистр (`rest.rs:69`) | ✅ | — | — |
| timestamp | мс; правило `timestamp < serverTime + 1000 && serverTime - timestamp <= recvWindow` | Свои часы (`rest.rs:56-59`) | ⚠️ | 🟠9 | Причина «часы» для 700003/10073 |
| recvWindow | «under 5 seconds is recommended», «The maximum is 60 seconds»; 700005 «recvWindow must less than 60000» | `venue.recv_window_ms`, отказ при `> 60000` (`rest.rs:47-54`) | ⚠️ | 🟡1 | `>= 60000` — отказ |
| X-MEXC-APIKEY | «the access key from your API Key» | `rest.rs:124-126` | ✅ | — | — |
| Content-Type | необязателен; `application/json` можно; «do not use `application/x-www-form-urlencoded`» (700013) | Тела нет, заголовок не ставится | ✅ | — | — |
| Параметры POST/PUT/DELETE | «may go in the query string, the body, or both» (пересказ) | Только строка запроса | ✅ | — | — |
| HTTP-коды | 4XX — ошибка запроса; 401 — авторизация; 403 — WAF; 429 — лимит; 5XX — сторона MEXC (в [G]: «execution status is UNKNOWN») | `errors.rs:79-101` только если тело не JSON; JSON-ошибка — по полю code (`rest.rs:160-163`) | ❌ | 🔴3 | 418/429/403 — по статусу всегда |
| Лимиты | IP 300 / 10 с, UID 500 / 10 с, независимо; ордер+отмена — UID 12/с; 500 открытых ордеров | Своего счётчика веса нет; темп — настройки (`fees.rs:78`, `order_state.rs:36`) | ⚠️ | 🟠1 | Учёт веса IP |
| Retry-After | «Responses with 418 or 429 include a `Retry-After` header… in seconds» | Заголовки не читаются (`gateway/http.rs` → `(status, text)`) | ❌ | 🔴3 | Читать Retry-After |
| Бан | «from a minimum of 2 minutes to a maximum of 3 days»; FAQ — «after 10 minutes» (противоречие доки) | Текст в `errors.rs:86-93` | ✅ (текст) | — | — |
| Формат ответа | Общей схемы нет: объект/массив без `code` или обёртка `{code, data, msg}` (tradeFee: `code: 0`, selfSymbols: `code: 200`) | `rest.rs:146-167` | ✅ / ⚠️ | 🟡16 | Ошибка при `code` ≠ 0/200 без `msg` |
| Резервный адрес | в документации нет | Только при `Fail::Connect` (и таймауте для Op::Other) (`rest.rs:132-134`, `171-173`) | ✅ | — | — |

### 2. Коды ошибок (полный список [L] …/error-code против `errors.rs:31-49`)

| Код MEXC | Текст в доке (дословно) | У нас | Итог |
|---|---|---|---|
| −2011 | «Unknown order sent» | BadResponse (`:43`); у Trade → OutcomeUnknown через `unsure` | ✅ по смыслу |
| 26 | «operation not allowed» | по умолчанию (Rejected/BadResponse) | 🟡 |
| 400 | «api key required» (на деле MEXC шлёт 400 и на «duplicate client order id», решение 114) | по умолчанию | 🟡 |
| 401 | «No authority» | InsufficientPermissions (`:35`) | ✅ |
| 403 | «Access Denied» | по умолчанию (в теле), InsufficientPermissions (без тела) | 🟡 |
| 429 | «Too Many Requests» | **по умолчанию — не RateLimited** | ❌ 🔴3 |
| 500 / 503 / 504 | «Internal error» / «service not available, please try again» / «Gateway Time-out» | Trade/Withdraw/Move → Timeout → OutcomeUnknown; Other → Unreachable (`:40-41`) | ✅ |
| 602 | «Signature verification failed» | InvalidCredentials | ✅ |
| 10001, 10015 | «user does not exist», «user id cannot be null» | по умолчанию | 🟡 |
| 10007 | «bad symbol» | по умолчанию | ✅ |
| 10072 | «invalid access key» | InvalidCredentials | ✅ |
| 10073 | «invalid Request-Time» | по умолчанию | 🟡 (🟠9) |
| 10095–10097, 10102 | «amount cannot be null», «amount decimal places is too long», «amount is error», «amount cannot be zero or negative» | WithdrawRejected (по умолчанию) | ✅ (🟠7 — 10096) |
| 10098 | «risk control system detected abnormal» | по умолчанию | ✅ |
| 10099, 10100, 10103, 10200–10211 | переводы между счетами | по умолчанию (Move → BadResponse → OutcomeUnknown через `unsure`) | ⚠️ для Move отказ = «неизвестно» — перевод не используется (`mod.rs:154-162`) |
| 10101 | «Insufficient balance» | InsufficientBalance (`:38`) | ✅ |
| 10212 | «This withdrawal address is not on the commonly used address list or has been invalidated» | WithdrawRejected | ✅ |
| 10216, 10219, 10222, 10232, 10259, 10268 | адрес/монета/запись потока | по умолчанию | ✅ |
| 10265 | «Due to risk control, withdrawal is unavailable, please try again later» | WithdrawRejected | ✅ |
| 11444 | «This feature is temporarily unavailable…» | Timeout → OutcomeUnknown (`:40`) | ⚠️ 🟡3 |
| 20001 | «subsystem is not supported» | по умолчанию | ✅ |
| 20002 | «Internal system error please contact support» | Timeout/Unreachable | ✅ |
| 22222 | «record does not exist» | по умолчанию | 🟡 |
| 30000, 30001, 30016, 30018, 30019, 30041 | «suspended transaction for the symbol», «The current transaction direction is not allowed…», «trading disabled», «market order is disabled», «api market order is disabled», «current order type can not place order» | OrderRejected | ✅ (🟡2) |
| 30002, 30003 | «The minimum transaction volume cannot be less than :», «…maximum… greater than :» | OrderRejected | ✅ |
| 30004, 30005 | «Insufficient position», «Oversold» | InsufficientBalance (`:38`) | ✅ |
| 30010, 30014, 30021, 30025, 30026 | цена/символ/ID | OrderRejected | ✅ |
| 30020 | «no permission for the symbol» | InsufficientPermissions | ✅ |
| 30027, 30028 | «…maximum position limit, the buying is suspended», «…platform risk control, the selling is suspended» | OrderRejected | ✅ |
| 30029, 30032 | «Cannot exceed the maximum order limit», «Cannot exceed the maximum position» | OrderRejected | ✅ |
| **30035** (новый 2026-10-23) | «You have triggered the order limit, please contact us by submitting a ticket.» | OrderRejected | 🟡2 (стоит паузы площадки) |
| 33333, 44444 | «param is error», «param cannot be null» | по умолчанию | ✅ |
| 60005 | «your account is abnormal» | по умолчанию | 🟡2 |
| 70011 | «Pair user ban trade apikey» | InsufficientPermissions | ✅ |
| 700001, 700002 | «API-key format invalid», «Signature for this request is not valid» | InvalidCredentials | ✅ |
| 700003 | «Timestamp for this request is outside of the recvWindow» | InvalidCredentials | ⚠️ 🟠9 |
| 700004 | «Param 'origClientOrderId' or 'orderId' must be sent…» | по умолчанию | ✅ |
| 700005 | «recvWindow must less than 60000» | по умолчанию | 🟡1 |
| 700006 | «IP non white list» | IpNotAllowed | ✅ |
| 700007 | «No permission to access the endpoint» | InsufficientPermissions | ✅ (🟠2) |
| 700008 | «Illegal characters found in parameter» | по умолчанию | ✅ |
| 730xxx, 140001/140002 | субаккаунты, ключи (в т.ч. 730703 «…a maximum of 10 IPs…») | по умолчанию | не используются |
| 157005 | «Withdrawal is currently locked by withdrawal protection.» | WithdrawRejected | ✅ |

### 3. Рынок и справочник пар

| Эндпоинт | Офдока [L] | У нас | Итог | Риск | Исправить |
|---|---|---|---|---|---|
| `GET /api/v3/exchangeInfo` | Вес IP 25; параметры `symbol` / `symbols` (через запятую) — необязательны; поля пары: `status` «1 - online, 2 - Pause, 3 - offline», `baseAssetPrecision`/`quotePrecision`/`quoteAssetPrecision` — целые, `baseSizePrecision` — минимальное количество, `quoteAmountPrecision` — минимальная сумма лимитного, `quoteAmountPrecisionMarket`, `maxQuoteAmount`, `maxQuoteAmountMarket`, `orderTypes`, `isSpotTradingAllowed` («API spot trading»), `permissions`, `tradeSideType` «1 - All, 2 - buy order only, 3 - Sell order only, 4 - Close», `filters[PERCENT_PRICE_BY_SIDE]`, `contractAddress`, `st`, `makerCommission`/`takerCommission`, новые `conceptPlateIds`/`conceptPlates` | `symbols.rs:81` без параметров (все пары); разбор `symbols.rs:195-226`: status `"1"|"ENABLED"`, isSpotTradingAllowed, tradeSideType, шаги по решению 107, min/max сумма, min количество. `orderTypes`, `filters`, `st`, `permissions`, `contractAddress` не читаются | ✅ по решению 107 | 🟡7 | Проверять `orderTypes` и фильтр цены до отправки |
| Шаг цены/количества | **в документации нет** (tickSize/stepSize) | 10^−quotePrecision / 10^−baseAssetPrecision (`symbols.rs:214-215`), страховка — order/test | ✅ решение 107 | — | — |
| `GET /api/v3/selfSymbols` | SPOT_ACCOUNT_READ, вес IP 1, параметров нет; ответ `{"code":200,"data":[…],"msg":null}` | `symbols.rs:82`, `162-174` | ✅ | 🟡12 | — |
| `GET /api/v3/tradeFee` | SPOT_ACCOUNT_READ, «Weight: 20 (IP)»; `symbol` YES; ответ `{"data":{"makerCommission":0E-18,"takerCommission":0.000500000000000000,"rpiTakerCommission":null},"code":0,"msg":"success","timestamp":…}` | `fees.rs:103-110`; числа через текст (`num.rs:35-48`) | ✅ разбор / ⚠️ вес | 🟠1, 🟡4 | Комментарий, бюджет веса |

### 4. Ордера

| Эндпоинт | Офдока [L] | У нас | Итог | Риск | Исправить |
|---|---|---|---|---|---|
| `POST /api/v3/order` | SPOT_DEAL_WRITE; «Shared with the cancel order endpoint, 12 requests/second»; `symbol` YES, `side` ENUM YES, `type` ENUM YES, `quantity` DECIMAL NO, `quoteOrderQty` DECIMAL NO, `price` DECIMAL NO, `newClientOrderId` STRING NO, `stpMode` NO, `recvWindow` «Max 60000», `timestamp` YES. LIMIT — `quantity`+`price`; MARKET — `quantity` или `quoteOrderQty`; для IMMEDIATE_OR_CANCEL / FILL_OR_KILL обязательные поля **в документации нет** | `trade.rs:120-127`: symbol, side, type (`venue.order_type`), quantity, price, newClientOrderId | ✅ | 🟠3 | — |
| Типы ордера | Enum [L]: «LIMIT», «MARKET», «LIMIT_MAKER», «IMMEDIATE_OR_CANCEL», «FILL_OR_KILL», «STOP_MARKET_ORDER (Query only)». Живой `orderTypes` [R]: LIMIT, MARKET, LIMIT_MAKER | настройка `venue.order_type` (`trade.rs:103`) | ✅ решение 107 | 🔴2 при LIMIT | — |
| `newClientOrderId` | Длина, символы, уникальность — **в документации нет**; живой ответ — «duplicate client order id» (решение 114) | `b|s` + 9 цифр мс + `_` + сделка, буквы/цифры/`_`, до 32 (`trade.rs:33-34`, `108-119`) | ✅ | — | — |
| Ответ | `orderId`, `orderListId`, `price`, `origQty`, `type`, `side`, `stpMode`, `transactTime` — без статуса | Берём только `orderId` (`trade.rs:147`) | ✅ / ⚠️ | 🟠3 | — |
| `POST /api/v3/order/test` | Вес IP 1; «Test order requests without submitting them to the matching engine»; параметры как у new order; ответ `{}` | `trade.rs:129-144`, Op::Other | ✅ | 🟡8 | — |
| `GET /api/v3/order` | SPOT_DEAL_READ; вес IP 2; `symbol` YES, `origClientOrderId` NO, `orderId` NO; ответ: `orderId`, `clientOrderId`, `price`, **`Qty`**, `executedQty`, `cumulativeQuoteQty`, `status`, `timeInForce`, `type`, `side`, `stopPrice`, `time`, `updateTime`, `isWorking`, `stpMode`, `cancelReason`, `origQuoteOrderQty`; запросы — «up to 7 days» | `order_state.rs:152-157`; разбор `order_state.rs:82-88`: `origQty`→`Qty`→`req.qty`, `executedQty`, `cumulativeQuoteQty`→`cummulativeQuoteQty` | ✅ разбор / ❌ обработка сбоя | 🔴2, 🟡9 | Повтор до срока, затем отмена |
| Статусы ордера | «NEW», «FILLED», «PARTIALLY_FILLED», «CANCELED», «PARTIALLY_CANCELED» | FINAL = FILLED, CANCELED, PARTIALLY_CANCELED (`order_state.rs:32`) | ✅ | — | — |
| `DELETE /api/v3/order` | SPOT_DEAL_WRITE; 12/с общий с постановкой; `symbol` YES, `orderId` NO, `origClientOrderId` NO, `newClientOrderId` NO; «Either `orderId` or `origClientOrderId` must be sent.»; ответ с `status`, `executedQty`, `cumulativeQuoteQty` | `order_state.rs:49-53` по `orderId`; ответ не разбирается — итог читается запросом | ✅ | 🟠4 (нет отмены по origClientOrderId) | — |
| `GET /api/v3/myTrades` | SPOT_ACCOUNT_READ; вес IP 10; `symbol` YES, `orderId` NO, `startTime`/`endTime` NO (не старше месяца), **`limit` NO — по умолчанию 10, максимум 1000**; ответ: `id`, `orderId`, `price`, `qty`, `quoteQty`, `commission`, `commissionAsset`, `isBuyer`, `isMaker`, `isSelfTrade`, `clientOrderId` | `order_state.rs:120-149` без `limit`; сумма `commission`, одна `commissionAsset` | ❌ | 🔴1 | `limit`, сверка с `executedQty` |

### 5. Аккаунт и ключ

| Эндпоинт | Офдока [L] | У нас | Итог | Риск | Исправить |
|---|---|---|---|---|---|
| `GET /api/v3/account` | SPOT_ACCOUNT_READ, вес IP 10; ([G]: «rate limit: 2 times/s»); `balances[]`: `asset`, `free`, `locked`, **`available`** (новое в [L]); `canTrade`, `canWithdraw`, `canDeposit`, `accountType`, `permissions` | `account.rs:35-48`: `asset`, `free`, `locked` | ✅ / ⚠️ | 🟡6, 🟡19 | Сверить `available` vs `free` |
| `GET /api/v3/apiKeyInfo` | SPOT_ACCOUNT_READ, вес IP 1 (задокументирован 2026-08-24); `accessKey` YES; ответ: `note`, `accessKey` (в таблице — `apikey`), `createTime`, `permissions` (строка через запятую), `ipWhiteList` (строка, формат не описан), `status` (VALID, DELETE, FROZEN), `remainingValidity` (−999 бессрочно, 0 истёк, >0 дней) | `account.rs:51-113`: status, remainingValidity, permissions, ipWhiteList | ✅ разбор / ❌ набор прав | 🟠2 | Добавить SPOT_DEAL_READ; сверить имена прав вывода |
| Срок ключа без IP | «The validity of an API Key without a linked IP address is 90 days» | В шапке `account.rs:14` | ✅ | — | — |
| Белый список IP | FAQ: «Each API Key can be linked to a maximum of 10 IP addresses» | не проверяется (не нужно) | — | — | — |

### 6. Кошелёк

| Эндпоинт | Офдока [L] | У нас | Итог | Риск | Исправить |
|---|---|---|---|---|---|
| `GET /api/v3/capital/config/getall` | SPOT_WITHDRAW_READ, вес IP 10, параметров нет; монета: `coin`, `name`, `networkList[]`; сеть: `depositEnable`, `withdrawEnable`, `withdrawFee`, `withdrawMin`, `withdrawMax`, `withdrawIntegerMultiple` (шаг суммы вывода — пересказ), `minConfirm`, `depositPreConfirms` (новое 2026-05-29), `name`, `network` (устаревшее имя, «scheduled for removal» — пересказ), `netWork` (код для нового вывода), `contract`, `sameAddress`, `withdrawTips`, `depositTips`, `depositDesc` | `wallet.rs:107-180`: `coin`, `network`, `netWork` (нет — `network`), `contract` (только у BSC), `depositEnable`/`withdrawEnable` (boolean, иначе сеть пропускается), `withdrawFee`, `withdrawMin`, `withdrawMax` (32 девятки → Decimal::MAX), `minConfirm` | ✅ / ⚠️ | 🟠7, 🟡13 | `withdrawIntegerMultiple` — по живому ответу |
| `POST /api/v3/capital/withdraw` | SPOT_WITHDRAW_WRITE, вес IP 1; `coin` YES, `withdrawOrderId` NO, `netWork` NO, `contractAddress` NO, `address` YES, `memo` NO («Required only if…» — пересказ), `amount` string YES, `remark` NO; ответ `{"id": "…"}`; `netWork` — «from the `networkList`… config/getall» (пересказ) | `transfers.rs:37-80`: coin, withdrawOrderId, netWork, address, amount; ответ `id` | ✅ | 🟠3, 🟠5 | — |
| Старый `withdraw/apply` | «Withdraw(previous,offline soon)» | не используется | ✅ | — | — |
| `GET /api/v3/capital/withdraw/history` | SPOT_WITHDRAW_READ, вес IP 1; `coin`, `status`, `limit` (default 1000, max 1000), `startTime` (по умолчанию −7 дней), `endTime`; окно ≤ 90 дней; ответ: `id`, `txId`, `coin`, `network` («may omit» у многосетевых — пересказ), `address`, `amount`, `transferType` (0 внешний, 1 внутренний), `status`, `transactionFee`, `confirmNo`, `applyTime`, `remark`, `memo`, `transHash`, `updateTime`, `withdrawOrderId` | `transfers.rs:84-127`: `limit=1000`, поиск по `id`; статус, `remark`, `txId`/`transHash`, `transactionFee`, `amount`, `network` | ✅ | 🟠5, 🟡18 | Поиск по `withdrawOrderId` |
| Статусы вывода | «1 APPLY», «2 AUDITING», «3 WAIT», «4 PROCESSING», «5 WAIT_PACKAGING», «6 WAIT_CONFIRM», «7 SUCCESS», «8 FAILED», «9 CANCEL», «10 MANUAL» | 1–6, 10 → Pending; 7 → Sent; 8, 9 → WithdrawRejected (`transfers.rs:104-125`) | ✅ полный список | — | — |
| `GET /api/v3/capital/deposit/hisrec` | SPOT_WITHDRAW_READ, вес IP 10; `coin`, `status`, `startTime` (−7 дней), `endTime`, `limit` (default 1000, max 1000); ответ: `amount`, `coin`, `network`, `status`, `address`, `addressTag`, `txId`, `insertTime`, `unlockConfirm`, `confirmTimes`, `memo` | `transfers.rs:130-171`: `coin`; поиск по `txId`/`transHash` без учёта регистра | ✅ / ⚠️ | 🟠6 | Нормализация хеша |
| Статусы депозита | «1 SMALL» … «5 SUCCESS» … «9 PRE_SUCCESS», «10 INVALID», «11 RESTRICTED», «12 COMPLETED» | 5, 12 → Credited; 1–4, 6, 9, 11 → Sent; 7, 8, 10 → DepositNotCredited (`transfers.rs:150-169`) | ✅ полный список (решение 107 д) | — | — |
| `GET /api/v3/capital/deposit/address` | SPOT_WITHDRAW_READ, вес IP 10; `coin` YES, `network` NO; ответ — массив `coin`, `network` (в примере «BEP20(BSC)»), `address`, `memo` | `wallet.rs:255-293`: `coin` + отображаемое имя `network`; поиск по имени или коду | ✅ | 🟡13 | — |
| `POST /api/v3/capital/deposit/address` | SPOT_WITHDRAW_WRITE, вес IP 1; `coin` YES, `network` YES | не используется | — | 🟡17 | — |
| Мин. депозит | **в документации нет** | `venue.deposit_min_absent_ok` (`wallet.rs:155-157`, решение 107 в) | ✅ | — | — |

### 7. Поток аккаунта (listenKey)

| Что | Офдока [L] | У нас | Итог | Риск |
|---|---|---|---|---|
| Создать | `POST /api/v3/userDataStream`, SPOT_ACCOUNT_READ, параметров нет; ответ `{"listenKey": "…"}`; «The stream will close 60 minutes after creation unless a keepalive is sent.»; вес — **в документации нет** | `user_key.rs:30-42` | ✅ | 🟠10 |
| Список | `GET /api/v3/userDataStream` → `total`, `listenKey[]`, `available` | не используется | — | — |
| Продлить | `PUT …`, `listenKey` YES; «Extends the validity to 60 minutes from the time of this call. It is recommended to send a request every 30 minutes.» | `user_key.rs:45-49`, раз в `venue.user_stream_keepalive_s` (< 3600, `user_stream.rs:113-121`) | ✅ | — |
| Закрыть | `DELETE …`, `listenKey` YES | `user_key.rs:52-55`, ошибка не проверяется | ✅ | 🟠10 |
| Лимиты | «maximum of 60 listen keys» на UID; «5 websocket connection» на ключ; «A single connection is only valid for 24 hours» | Один ключ на сессию, новый на каждое переподключение | ⚠️ | 🟠10 |
| Адрес | «/ws?listenKey=listenKey», пример `wss://wbs-api.mexc.com/ws?listenKey=…` | `user_stream_conn.rs:42` — `venue.ws_url` + `?listenKey=` | ✅ | — |
| Ключ в журналах | — | Путь не пишется в сырой журнал (`rest.rs:150`), ключ заменяется `***` (`user_stream_conn.rs:41`) | ✅ | — |

### 8. WebSocket

| Канал/правило | Офдока [L] / [P] | У нас | Итог | Риск |
|---|---|---|---|---|
| Адрес | `wss://wbs-api.mexc.com/ws` | `venue.ws_url` (`ws.rs:111`) | ✅ | — |
| Подписок на соединение | «maximum of 30 subscriptions»; сверх — «Any subscription will be invalid after the limit is exceeded» | `venue.ws_symbols_per_conn` ≤ 30 (`market.rs:29`, `46-54`) | ✅ | — |
| Запросов в секунду | 100 запросов/с (введение, пересказ); частые переподключения — блок IP | `venue.ws_subs_per_sec` без верхнего предела (`ws.rs:109-115`) | ⚠️ | 🟡11 |
| Подписка | `{"method":"SUBSCRIPTION","params":["…"]}` → `{"id":0,"code":0,"msg":"<канал>"}`; «If the `msg` in the response matches the corresponding request field, it indicates that the request was sent successfully» | `ws_conn.rs:78-84` по одному каналу; ответ `code ≠ 0` → стакан пары снят (`ws_conn.rs:147-154`) | ✅ | — |
| PING | `{"method":"PING"}` → `{"id":0,"code":0,"msg":"PONG"}` ([G]); FAQ [L]: «a Pong reply is required upon receiving the Ping message sent from the server»; частота — **в документации нет** | Свой PING раз в `venue.ws_ping_ms` (`ws_conn.rs:85-90`); ответ Pong на Ping-кадр (`ws_conn.rs:113-118`) | ✅ | 🟡11 |
| Разрывы | без подписки — 30 с; подписка без данных — 1 минута; соединение ≤ 24 часа | Тишина `venue.ws_silence_ms` (`ws_conn.rs:91-93`); переподключение с паузой (`ws.rs:57-94`), стаканы снимаются (`ws.rs:65-67`) | ✅ | — |
| Частичный стакан | `spot@public.limit.depth.v3.api.pb@<symbol>@<level>`, уровни 5, 10, 20; частота — **в документации нет**; пример: `channel`, `symbol`, `sendTime`, `publicLimitDepths{asks[], bids[{price, quantity}], eventType, version, lastOrderCreateTime}` | `ws_conn.rs:38-40`; `venue.depth_levels` ∈ {5,10,20} (`market.rs:31`, `55-61`); разбор `depth.rs:69-107` | ✅ | 🟠8, 🟡10 |
| Обёртка [P] | `PushDataV3ApiWrapper`: `channel`=1, oneof body (303 publicLimitDepths, 304 privateOrders, 307 privateAccount …), `symbol`=3 optional, `symbolId`=4, `createTime`=5, `sendTime`=6 | `proto.rs:13-40` | ✅ дословно | — |
| `PublicLimitDepthsV3Api` [P] | asks=1, bids=2, eventType=3, version=4, lastOrderCreateTime=5; item price=1, quantity=2 (string) | `proto.rs:42-62` (без 5 — пропускается prost) | ✅ | — |
| `spot@private.account.v3.api.pb` | Подписка `{"method": "SUBSCRIPTION", "params": ["spot@private.account.v3.api.pb"]}`; поля `vcoinName`, `coinId`, `balanceAmount`, `balanceAmountChange`, `frozenAmount`, `frozenAmountChange`, `type`, `time` (long, «Settlement time» — пересказ) | `user_stream_conn.rs:32`, `54`; `proto.rs:64-82`; `account_update.rs:50-105` | ✅ | — |
| Типы изменения | «WITHDRAW», «WITHDRAW_FEE», «DEPOSIT», «DEPOSIT_FEE», «ENTRUST», «ENTRUST_PLACE», «ENTRUST_CANCEL», «TRADE_FEE», «ENTRUST_UNFROZEN», «SUGAR», «ETF_INDEX» | `account_update.rs:37-47` — все перечислены; хеша транзакции нет — `tx_hash: None` | ✅ | — |
| Protobuf | «The current websocket push uses the protobuf format.» | двоичные кадры — prost, текстовые — JSON | ✅ | — |

---

## Логические проблемы реализации (аналитика, сверх расхождений с докой)

1. **Частичное исполнение и пыль.** `order_state::result` (`order_state.rs:89-95`) считает «частично» по `executedQty < origQty`; остаток снят отменой (LIMIT) или биржей (IOC) — ✅. Округление количества — вниз по шагу (`trade.rs:50`), цена покупки — вниз, продажи — вверх (`trade.rs:51-59`): худшая цена не нарушается ✅. Пыль после продажи (остаток меньше `baseSizePrecision` / суммы меньше `quoteAmountPrecision`) отсекается проверкой до ордера (`trade.rs:69-102`) — сделка отказывает, а не теряет; в ядре это «застрявшая монета» по объёму пыли — ожидаемо.
2. **Повторы ордеров.** Свой ID у каждой попытки (решение 114) ✅. Дыра — 🟠3: успешный HTTP без `orderId` уходит как BadResponse и разрешает повтор.
3. **Неизвестный итог.** Постановка: OutcomeUnknown → один поиск по origClientOrderId (🟠4). Ожидание: первый сбой запроса — OutcomeUnknown без отмены (🔴2). Вывод: поиска нет (🟠5).
4. **Гонки.** `Subs` — подписка и пересподписка под замком ✅. `Fees` и `Symbols` — RwLock, читатели видят старое или новое целиком ✅. Справочник пар обновляется и фоном, и в `trade` (🟡14) — двойной exchangeInfo возможен, не опасен.
5. **При 429.** См. 🔴3: не распознаётся, Retry-After не соблюдается; бан IP перекрывает и рынок (REST), и ордера.
6. **При перезапуске.** Ордер LIMIT, оставшийся после падения бота посреди `settle`, никто не отменяет — сверка по открытым ордерам (`GET /api/v3/openOrders`) или `DELETE /api/v3/order/all` на старте не делается. Старые listenKey истекут сами за 60 мин ✅. Комиссии и справочник пар — заново при подписке ✅.
7. **Протухший listenKey.** Продление не прошло → сессия рвётся, ключ закрывается, новый ключ (`user_stream_conn.rs:63-66`) ✅. Если MEXC закрыл поток раньше продления (24 ч) — `Close`/конец потока → переподключение с новым ключом ✅ (см. 🟠10 про число ключей).
8. **Лимит 12 ордеров/с на UID** (постановка + отмена, [L] change log 2026-03-11) — своего счётчика нет; при нескольких сделках сразу (BingX ↔ MEXC и MEXC ↔ DEX одновременно) плюс пробный ордер — упор маловероятен, но 429 здесь — 🔴3.

## Что проверить на живых записях (без денег)

1. Ответ `apiKeyInfo`: точные имена прав (SPOT_WITHDRAW_* или иные), формат `ipWhiteList` (🟠2).
2. `config/getall`: значения `withdrawIntegerMultiple` у сетей BSC (🟠7).
3. `deposit/hisrec`: формат `txId` для BSC (🟠6).
4. Тело ответа MEXC на 429 и наличие Retry-After (🔴3).
5. `account`: `free` против `available` (🟡6).
6. `myTrades` по ордеру с >10 сделками — сколько приходит без `limit` (🔴1).
7. Как быстро `GET /api/v3/order` видит только что поставленный ордер (−2011 сразу после постановки?) (🔴2).
