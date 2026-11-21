# CoinEx — анализ для подключения (без кода) — 2026-11-05

Источник — только официальная документация CoinEx API v2 (https://docs.coinex.com/api/v2/), английская версия
(китайская `/zh/` сверялась по странице вывода). Чек-лист — `docs/notes/cex-integration-lessons-2026-11-04.md`.

**Что не удалось проверить:** живые ответы API. С сервера разработки `api.coinex.com` и `docs.coinex.com` закрыты
прокси (CONNECT 403), репозитории `github.com/coinexcom/*` (coinex_exchange_api, coinex_api_demo) не подключены к
сессии. Поэтому всё ниже — по тексту документации; пункты с пометкой **[проверить живым]** надо записать на боте
(`tests/data/...`) до кода. Примеры в документации CoinEx местами невалидный JSON и расходятся с таблицами полей —
это отдельный риск (урок 2).

## 1. Спот API, адреса, подпись

- Спот есть: раздел Spot (https://docs.coinex.com/api/v2/spot/market/http/list-market). API v1 выключен 25.09.2024
  (https://docs.coinex.com/api/v2/changelog) — работать только с v2.
- REST: `https://api.coinex.com/v2` (https://docs.coinex.com/api/v2/guide).
- WebSocket: спот `wss://socket.coinex.com/v2/spot`, фьючерсы `wss://socket.coinex.com/v2/futures` (там же).
  **Ответы WS сжаты**: «The response from the WS server is compressed using zip and must be decompressed first» —
  каждый кадр надо распаковывать (какой именно формат — deflate/gzip — **[проверить живым]**).
- Подпись REST (https://docs.coinex.com/api/v2/authorization): заголовки `X-COINEX-KEY` (access_id),
  `X-COINEX-SIGN`, `X-COINEX-TIMESTAMP` (мс); строка = `METHOD + request_path(с ?query) + body + timestamp`;
  HMAC-SHA256 ключом secret_key, **hex в нижнем регистре, 64 символа**. Окно — `X-COINEX-WINDOWTIME`, по умолчанию
  5000 мс (guide) → настройка `venue.recv_window_ms`.
- **Passphrase нет.**
- Подпись WS: строка = только timestamp; метод `server.sign` {access_id, signed_str, timestamp}
  (https://docs.coinex.com/api/v2/common/ws/sign); ошибка — код 21002.
- Формат ответа: HTTP 200 + `{code, data, message}`, `code = 0` — успех; не 200 — ответ шлюза CoinEx, тело не
  разбирать как JSON, а записать сырым (https://docs.coinex.com/api/v2/error).

## 2. Справочник монет и сетей

- Монеты и сети: `GET /assets/all-deposit-withdraw-config` (все, без параметров) и
  `GET /assets/deposit-withdraw-config?ccy=` (одна монета — урок 9 / решение 113)
  (https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-all-deposit-withdrawal-config,
  …/get-deposit-withdrawal-config). Поля сети `chains[n]`: `chain`, `min_deposit_amount`, `min_withdraw_amount`,
  `deposit_enabled`, `withdraw_enabled`, `deposit_delay_minutes`, `safe_confirmations`,
  `irreversible_confirmations`, `deflation_rate`, `withdrawal_fee`, `withdrawal_precision`, `memo`,
  `is_memo_required_for_deposit`, `explorer_asset_url`. На уровне монеты `asset.deposit_enabled`,
  `asset.withdraw_enabled`, `asset.inter_transfer_enabled`, `asset.is_st`.
- **Название сети BSC — в документации не указано** (в примерах только CSC и «Ethereum (ERC20)»). Только из
  живого справочника → `venue.bsc_network` **[проверить живым]**.
- **Адреса контракта отдельным полем в справочнике ввода/вывода нет.** Есть два источника:
  - `GET /assets/info` → `chain_info[n].identity` — «на большинстве цепей это адрес контракта», пусто для
    нативных монет; `chain_info[n].chain_name` (https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-assets-info).
  - `explorer_asset_url` в справочнике ввода/вывода — адрес внутри ссылки обозревателя («Token address in blockchain
    explorer»). Разбирать адрес из URL — это гадание, не использовать как основной источник.
  - Риск: имена цепей в двух справочниках, судя по примерам, **разные** («Ethereum (ERC20)» в `/assets/info`
    против `chain` в deposit-withdraw-config). Как сопоставить сеть BSC между ними — в документации нет.
    Нужна отдельная настройка имени BSC для `/assets/info` (например `venue.bsc_chain_info_name`) или вопрос
    Тиграну. **[проверить живым]**
- Статус ввода/вывода по сети — `deposit_enabled` / `withdraw_enabled` (+ флаги монеты целиком).
- Комиссия вывода `withdrawal_fee`, минимум `min_withdraw_amount`, точность `withdrawal_precision`.
- Минимум депозита — `min_deposit_amount` прямо в справочнике (урок 21 проще, чем у Gate). Семантика null/"0" —
  в документации нет. Депозит ниже минимума — статус `too_small`, «cannot be automatically credited, and a second
  deposit is needed» (https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-deposit-history).
- memo: `memo` и `is_memo_required_for_deposit`; описание поля противоречит имени («Whether a memo is required for
  withdrawal») — урок 2. Для BSC memo, скорее всего, не нужен, но решать по живому ответу.
- `deflation_rate` — у монет с налогом на перевод; в документации смысл одной строкой. Может пригодиться для
  налога токена, но только после подтверждения.
- Размер полного справочника — не указан; gzip запроса — в документации не сказано (урок 28) **[проверить живым]**.

## 3. Вывод и депозит

- Вывод: `POST /assets/withdraw` {ccy, chain, to_address, amount, memo, extra, withdraw_method=`on_chain`,
  fee_ccy, remark} → `withdraw_id`, `status`, `tx_id`, `tx_fee`, `actual_amount`
  (https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/withdrawal).
  - «Withdrawal fee will be charged for each request, please reserve sufficient trading fee» — комиссия сверх суммы
    или из суммы — по тексту не однозначно (`actual_amount` в ответе) **[проверить живым]**.
  - **Белый список адресов обязателен**: «The withdrawal address needs to be added to the IP whitelist via Developer
    Console» (формулировка кривая); FAQ (https://docs.coinex.com/api/v2/faq, код 3310): адреса вывода по API —
    в белый список на странице API-ключа, тогда вывод без вторичной проверки. Значит адрес кошелька DEX (Маппинг
    кошельков) и адреса депозитов других бирж надо заранее внести в кабинете.
  - Права: у ключа флаг `withdraw_enabled` (видно только в описании ключей субаккаунтов).
  - Статусы `withdraw_status`: created (ждёт подтверждения по email), audit_required, audited, processing,
    confirming, finished, cancelled, cancellation_failed, failed (https://docs.coinex.com/api/v2/enum).
    В примерах ответа — `"audit"`, которого нет в перечне — урок 2 **[проверить живым]**.
  - Статус по своему id: `GET /assets/withdraw?withdraw_id=` (https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-withdrawal-history).
    Поиска по своей метке нет: `remark` есть только в ответе — искать по remark перебором последних страниц.
  - Отмена вывода — `cancel-withdrawal` (с 17.06.2026 требует отдельных прав, changelog).
- Адрес депозита: `GET /assets/deposit-address?ccy=&chain=` → `address`, `memo`; адрес можно сменить
  (`update-deposit-address`) — кэш адреса может устареть (урок 25)
  (https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/get-deposit-address).
- История депозитов: `GET /assets/deposit-history?ccy=&tx_id=` — поиск по хешу есть; поля `status`, `confirmations`,
  `amount`, `actual_amount`, `chain`.
  - Статусы `deposit_status`: processing, confirming, cancelled, finished, too_small, exception. В примере —
    `"finish"` (не `finished`) — урок 2.
  - **Статуса «предварительно зачислено» (как MEXC PRE_SUCCESS) нет в документации** → `withdraw_hold` (урок 22)
    не по чему реализовывать; есть `deposit_delay_minutes` в справочнике — смысл не описан, вопрос Тиграну.
  - В примере запроса — `coin=CET`, в таблице — `ccy` (урок 2).
- На какой счёт зачисляется депозит — **в документации прямо не сказано**. Счета перевода — только SPOT, MARGIN,
  FUTURES, и «either the outgoing or incoming account … must be a spot account»
  (https://docs.coinex.com/api/v2/assets/transfer/http/transfer); отдельного funding-счёта нет. Значит, по всей
  видимости, депозит и вывод — со спота и внутренний перевод не нужен, но это вывод из структуры, а не факт
  документации **[проверить живым]**.

## 4. Ордера

- `POST /spot/order` {market, market_type=`SPOT`, side, type, amount, price, ccy, client_id, is_hide, stp_mode}
  (https://docs.coinex.com/api/v2/spot/order/http/put-order).
- IOC — **это тип ордера, а не timeInForce**: `type = ioc` (и `fok`, `maker_only`, `limit`, `market`)
  (https://docs.coinex.com/api/v2/enum). Ошибка 3621 «can't be completely executed and has been canceled» (FOK).
  Цена для ioc обязательна или нет — в таблице не сказано **[проверить живым]**.
- client_id: буквы, цифры, `-`, `_`, до 32 байт. Уникальность — в документации нет (урок 6: новый у каждой попытки).
  Поиска ордера по client_id **нет** (только отмена `POST /spot/cancel-order-by-client-id`).
- Итог: ответ ордера и `GET /spot/order-status?market=&order_id=` — `status`, `filled_amount`, `filled_value`
  (средняя цена = filled_value / filled_amount), `base_fee`, `quote_fee`, `discount_fee` (комиссия в CET), ставки
  `maker_fee_rate`/`taker_fee_rate`. Отдельного поля «монета комиссии» нет — монета по тому, какое из трёх полей
  ненулевое. В сделках `GET /spot/user-deals` есть `fee` + `fee_ccy` (но фильтра по order_id нет).
- **Критично для IOC:** «after an order with no transactions is canceled, the server will not save the order, nor can
  the data of this order be queried via any endpoint» (order-status, finished-order). IOC с нулевым исполнением
  потом не найти никак — итог брать из ответа самого `POST /spot/order`; что ответ IOC содержит окончательное
  состояние — в документации не сказано **[проверить живым пробным ордером на минимальную сумму]**. Ответ
  «ордер не найден» после IOC = «не исполнился», а не ошибка — решение Тиграна.
- Правила пары `GET /spot/market` (до 10 пар через запятую или все): `min_amount` (мин. количество),
  `base_ccy_precision`, `quote_ccy_precision` (знаки после запятой), `status` (online / counting_down / bidding),
  `is_api_trading_available`, `delisted_at`, `maker_fee_rate`/`taker_fee_rate`. **Шага цены (tick) и шага количества
  отдельно нет — только число знаков; мин. суммы в котируемой монете нет; максимума нет.** Ошибки 3127 (меньше
  минимума), 3606/3615/3618 (отклонение цены от последней/индекса) — фильтры цены не проверяемы заранее.
  В примере ключ `is_pre_trading_available`, в таблице `is_pre_market_trading_available` — урок 2.
- **Пробного ордера (test order) нет** (в списке эндпоинтов и в changelog). `check_order` — только по правилам пары.

## 5. Стакан

- WS `depth.subscribe` {market_list: [[market, limit, interval, if_full], …]}; limit ∈ {5, 10, 20, 50}; interval
  (склейка) `"0"` и др. (https://docs.coinex.com/api/v2/spot/market/ws/market-depth).
  - `if_full = true` — каждый раз полный снимок N уровней; `false` — инкременты (`is_full` в кадре), размер "0" —
    удалить уровень; **checksum** — signed int32 CRC32 строки `bid1_price:bid1_amount:…:ask1_price:ask1_amount:…`
    (в таблице string, в примере число 2578768879 — не помещается в signed int32 → урок 2).
  - Номеров последовательности нет — разрыв ловится только checksum; полный снимок раз в минуту.
  - Пуш раз в 200 мс и только при изменении — `venue.book_stale_ms` не годится как «нет кадров = устарело»
    (как у KuCoin).
  - Рекомендация: `if_full=true`, limit 20/50 — без ресинков (как KuCoin level2Depth50), процессор — на распаковку.
- BBO: `bbo.subscribe` — real-time лучшая цена (https://docs.coinex.com/api/v2/spot/market/ws/market-bbo).
- REST снимок: `GET /spot/depth?market=&limit=&interval=` (https://docs.coinex.com/api/v2/spot/market/http/list-market-depth).
- **Лимиты подписок на соединение, сообщений в секунду, число соединений — в документации нет.** Ошибка WS 23002
  «Requests submitted too frequently». → `venue.ws_symbols_per_conn`, `venue.ws_subs_per_sec` — вопрос Тиграну /
  подбор по живому.
- Пинг: `server.ping` → `"pong"` (https://docs.coinex.com/api/v2/common/ws/ping). **Интервал и таймаут простоя — не
  указаны** → настройка `venue.ws_ping_s`, значение — по живому.

## 6. Комиссии

- `GET /account/trade-fee-rate?market_type=SPOT&market=` → maker_rate, taker_rate — **только по одной паре**, «no
  endpoint that returns fee rates for all markets» (https://docs.coinex.com/api/v2/account/fees/http/get-account-trade-fees).
  Фоновый круг по монетам подписки, пачками в пределах лимита (урок 26). Ставки также в `GET /spot/market` и в ответе
  ордера.
  - **Единицы неясны:** в примере fee-rate `taker_rate: "0.03"`, у `/spot/market` — `"0.002"`. 0.03 — это 3 % или
    0.03 %? Урок 2 **[проверить живым до расчёта]**.
- Скидка CET: `POST /account/settings {cet_discount_enabled}` — глобальный выключатель; при ордере не управляется
  (https://docs.coinex.com/api/v2/account/settings/http/modify-account-settings). **Прочитать текущее значение
  нельзя** (GET настройки нет). В ордере комиссия CET — `discount_fee`. Урок 23: расчёт по обычной ставке, факт — из
  ордера; какой размер скидки — в документации нет.

## 7. Лимиты запросов

- IP: 400 запросов/с. Пользователь, короткий цикл (по аккаунту, субаккаунты отдельно): размещение/правка спот-ордеров
  30/с, отмена 60/с, пакетная отмена 40/с, запрос ордеров 50/с, история ордеров 10/с, изменение счёта 10/с, запрос
  счёта 10/с, история счёта 10/с; пакет — каждая подзаявка = 1 (https://docs.coinex.com/api/v2/rate-limit).
- Длинный цикл (1 ч/4 ч/8 ч/24 ч) по «объёму и качеству» запросов — чисел нет; при срабатывании «low-speed mode»
  (отмены не ограничиваются).
- Превышение: код 4213 (а также 3008, 4001). Заголовки `X-RateLimit-Limit`, `X-RateLimit-Remaining`,
  `X-RateLimit-LongPeriod-{period}-Remaining`. **HTTP-статус и Retry-After — не описаны** → пауза группы на
  настраиваемое время (`venue.rate_pause_ms`), чтение Remaining — заранее тормозить.
- К какой группе относятся эндпоинты активов (deposit-withdraw-config, fee-rate, вывод) — на странице лимитов не
  перечислено полностью **[уточнить]**.

## 8. Приватный WS (поток аккаунта)

- Есть: после `server.sign` — `balance.subscribe {ccy_list: []}` → `balance.update` (ccy, available, frozen,
  updated_at) (https://docs.coinex.com/api/v2/assets/balance/ws/spot_balance) и `order.subscribe {market_list: []}`
  → `order.update` с event put/update/modify/finish (https://docs.coinex.com/api/v2/spot/order/ws/user-order).
  Поля комиссии в WS называются иначе, чем в REST (`base_ccy_fee`, `quote_ccy_fee`, `discount_ccy_fee`) — урок 2.
- Токена потока нет — авторизация подписью на том же адресе `wss://socket.coinex.com/v2/spot`.

## 9. Счета (панель «Балансы», урок 24)

- Спот `GET /assets/spot/balance` (ccy, available, frozen).
- Фьючерсы `GET /assets/futures/balance`.
- Маржа — страница get-marigin-balance; Earn (financial) `GET /assets/financial/balance`; кредитный счёт и AMM — свои
  страницы (https://docs.coinex.com/api/v2/assets/balance/http/get-spot-balance, сайдбар). Пути маржи/кредита/AMM
  не выписаны — при реализации открыть страницы.
- Funding-счёта у CoinEx нет (см. п. 3).

## 10. Права ключа

- Права ключа: торговля (`trade_enabled`) и вывод (`withdraw_enabled`), белый список IP (`ip_whitelist`), срок
  (`expires_at`, `is_expired`) — поля видны только в `GET /account/subs/api-detail` для ключей **субаккаунтов**
  (https://docs.coinex.com/api/v2/account/subs/http/get-sub-api). Права субаккаунта: FUTURES, MARGIN, AMM, API.
- **Проверить права своего ключа основного аккаунта через API — нет в документации.** «Проверить подключение»: подпись
  через `GET /account/info` (user_id) + разбор кодов 4005 (ключ, новый — 10 минут до активации), 4006 (подпись),
  4007 (IP не в белом списке), 4010/4017 (время), 4011/4115 (запрет). Право вывода и отсутствие фьючерсов — только
  описанием в UI («проверьте в кабинете») либо работать с ключом субаккаунта, у которого права читаются
  (вопрос Тиграну).
- Ключ может истекать (FAQ к 4005: «check whether an older key has expired») — показывать в UI.

## 11. Уроки чек-листа, важные для CoinEx

| Урок | Что у CoinEx | Как закрывать |
| --- | --- | --- |
| 1–2 | Таблицы и примеры расходятся (coin/ccy, finish/finished, audit, is_pre_*, confirmation(s), checksum, имена полей комиссии WS/REST, «IP whitelist» для адресов) | Всё — по записанным ответам; расхождения — Тиграну |
| 3 | Живых ответов нет (сеть закрыта) | Записать публичные на сервере бота до кода |
| 4 | Нет tick/шага — только precision; нет мин. суммы; нет пробного ордера | `order_rules::ready` по precision и min_amount; `venue.order_test_first` неприменимо |
| 5, 30 | IOC = `type: ioc` | Проверить живым минимальным ордером |
| 6 | client_id ≤ 32 байт, [A-Za-z0-9-_] | Новый на каждую попытку |
| 7 | Неисполненный IOC не сохраняется вообще | Итог — из ответа POST; «не найден» = не исполнился (решение Тиграна) |
| 8 | Нет поля контракта в справочнике сетей; адрес — `/assets/info.identity` с другим именем цепи | Два справочника + сопоставление имени цепи настройкой; без адреса — монета не торгуется |
| 9 | Есть запрос одной монеты (`?ccy=`), полный справочник — объём неизвестен | spawn_blocking, Arc, одна монета при recheck |
| 10 | Депозит, видимо, сразу на спот | Подтвердить живым; `venue.deposit_to_spot` не нужен |
| 11, 23 | Ставка — по одной паре; единицы под вопросом; CET-скидка не читается | Фоновый круг; худший случай; факт из ордера |
| 12 | Снимок N уровней или инкремент + checksum без номеров; лимиты подписок не документированы; ping без интервала | Снимок if_full=true; лимиты — настройки, значения по живому |
| 13 | Каждый кадр WS сжат | Один распаковщик на соединение, буфер переиспользовать; замер нагрузки (решение 109) обязателен |
| 15 | 4213, Remaining-заголовки, без Retry-After | Пауза группы настройкой |
| 16 | balance.update / order.update есть | Баланс для расчёта — из потока |
| 19, 29 | Права основного ключа не читаются | Вопрос Тиграну (субаккаунт?) |
| 21 | min_deposit_amount в справочнике | Без фонового запроса адресов; null — `venue.deposit_min_absent_ok` |
| 22 | Нет pre-credit статуса; есть `deposit_delay_minutes` | Вопрос Тиграну |
| 25 | Адрес депозита можно сменить | Кэш + сверка в предпроверке |
| 27 | «Счёт не открыт» — не описан | По живому |
| 28 | gzip REST и IPv4/IPv6 — не описаны | По живому |

## 12. Вердикт

**ПОДКЛЮЧАТЬ — условно**, после записи живых ответов. Спот есть, IOC есть, вывод и адрес депозита через API есть,
поток баланса и ордеров по WS есть, минимум депозита прямо в справочнике, funding-счёта нет.

Условия до кода (блокирующие): (1) в живом справочнике есть сеть BSC и у BEP20-токенов заполнен
`/assets/info.identity` с понятным соответствием имени цепи; (2) единицы `taker_rate`; (3) ответ IOC-ордера несёт
окончательный итог.

**Сложность: средняя** (ближе к высокой из-за пробелов документации). API собственный, не Binance-подобный: REST в
стиле KuCoin/Gate (`{code, data, message}`, пары `BTCUSDT`, IOC типом ордера), WS — JSON-RPC (`method/params/id`,
`server.sign`, `depth.subscribe`) со сжатием кадров; стакан-снимок N уровней — как KuCoin level2Depth50.

### Главные риски
1. **Адрес контракта** — не в справочнике сетей, а в `/assets/info.identity` с другим названием цепи; без надёжного
   сопоставления сопоставление монет по адресу (жёсткое правило) не выполнить.
2. **Неисполненный IOC не сохраняется** — итог только из ответа размещения; ошибка разбора = потерянный факт.
3. **Нет шага цены/количества и пробного ордера** — только число знаков и min_amount; фильтры отклонения цены
   (3606/3615/…) не проверяются заранее.
4. **Права своего ключа не читаются через API**, вывод — только на адреса из белого списка ключа (вносить руками).
5. **Пробелы WS**: лимиты подписок/соединений и интервал пинга не документированы, каждый кадр сжат — риск CPU
   (урок 13, решение 109); комиссии — единицы ставки под вопросом, состояние CET-скидки не читается.

### Вопросы Тиграну (в docs/TASKS.md при старте работ)
- Неисполненный IOC «не найден» = «не исполнился»: принимаем?
- Права ключа основного аккаунта не проверяются API — достаточно предупреждения в UI или работать через субаккаунт?
- `deposit_delay_minutes` — считать ли удержанием вывода?
- Включать ли CET-скидку (глобально, состояние не читается)?
