# venues/phemex — адаптер биржи Phemex (решение 168, только спот)

Контракт `gateway/contract.rs` целиком. Источник — только официальная документация Phemex:
<https://phemex-docs.github.io/> (исходники сайта — github.com/phemex-docs/phemex-docs.github.io, файлы
`source/includes/_generic.md`, `_spot.md`, `_withdraw.md`, `_transferV2.md`, `_uta.md`, `_contract.md`), старая
официальная документация github.com/phemex/phemex-api-docs (`Public-Spot-API-en.md` — таблицы полей пар и монет) и
официальный пример клиента github.com/phemex/phemex-python-api (`phemex/client.py` — подпись). Анализ —
`docs/notes/phemex-analysis-2026-11-05.md`, закрытие уроков — `docs/notes/phemex-integration-2026-11-05.md`.

## Главное отличие Phemex

Цены, доли и суммы — **масштабированные целые**: `…Ep` = цена × 10^priceScale, `…Ev` = значение × 10^valueScale
монеты, `…Er` = доля × 10^8, `…Rv` — настоящее значение строкой. Перевод — только `num.rs` (целочисленно, без f64;
значение, которое не ложится на масштаб ровно, — отказ). Масштаб монеты — из справочника `/public/products`
(`symbols.rs`): без него не читаются балансы, депозиты, комиссии.

## Файлы

| Файл | Что делает | Документация |
|---|---|---|
| mod.rs | контракт адаптера | — |
| ctx.rs | настройки, секреты (ключ и секрет; passphrase нет), журнал | #rest-request-header |
| sign.rs | HMAC SHA256 hex: путь + строка + expiry + тело; вход WebSocket | #endpoint-security-type, #user-authentication-2 |
| rest.rs, reply.rs, errors.rs | запросы, ответы {code,msg,data} и /md {error,id,result}, 429 — пауза, коды; отказы вывода (решение 253, дополнение t82): кодов адреса и аккаунта в таблице нет — без изменений | #rest-response-format, #rate-limits, #error-codes |
| num.rs | масштабированные целые ↔ Decimal | #price-ratio-value-scales |
| symbols.rs, symbols_parse.rs | пары (шаги Ev/Ep, пределы) и масштабы монет | #query-product-information |
| pairs.rs, pairs_watch.rs | состояние пар для ядра, обновление справочника | — |
| delist.rs | решение 226: делистинги — `timeline[3]` спот-пар Listed GET /public/products-plus (мс); `stStatus` в документации нет — не используется | #query-product-information-plus |
| subs.rs, market.rs, ws.rs, ws_conn.rs | подписка стаканов (≤ 20 на соединение, ≤ 5 соединений), ping; решение 202 — учёт свежести для сторожа ядра (gateway/feed_reg.rs), «переподписать пару» = orderbook.unsubscribe (всех) + orderbook.subscribe всех пар соединения, переподключение одного соединения | #subscribe-orderbook-2, #heartbeat-2, Unsubscribe OrderBook |
| depth.rs, local_book.rs, book.rs | кадры стакана, снимок + изменения, котировка; решение 213 — срок подтверждения тихой пары (`book_confirmed`) | #orderbook-message |
| book_print.rs | решение 213 по содержимому: GET /md/orderbook — первые `venue.depth_levels` уровней ответа и стакана в памяти для подтверждения тихой пары ядром; Решение 238: тот же запрос — снимок стакана для ордера (`snapshot`, контракт `quote_rest`), все уровни ответа | Query Order Book |
| fees.rs | ставки комиссии ключа по паре | #query-fee-rate-by-quote-currency |
| order_rules.rs, trade.rs, order_state.rs | ордер Limit + ImmediateOrCancel, свой clOrdID (UUID), итог; решение 264: ответ постановки — bizError ≠ 0 / ordStatus Rejected — отказ; отмены IOC/FOK нет (коды 11109/11110) | #place-order-http-put-prefered, #query-orders-by-order-id-or-client-order-id |
| market_sell.rs | решение 268: рыночная продажа (контракт trade_market_sell) — ордер ordType Market, qtyType ByBase на количество базовой монеты после отказа лимитного ордера «цена вне диапазона»; правила пары и разбор итога — те же, что у лимитного (order_rules.rs, trade.rs → place); код PriceOutOfRange рыночный ордер не отдаёт |
| wallet.rs, networks_crawl.rs, networks_parse.rs | сети монет по одной (chainCfg + asset/info), фоновый круг | #query-deposit-chain-settings, #query-withdraw-chain-settings |
| deposit_addr.rs, deposits.rs, transfers.rs, history.rs | адрес депозита, депозиты по хешу, вывод и его статус | #query-deposit-address-information, #create-withdraw-request |
| fund_move.rs | перевод CONTRACT → SPOT после депозита (по настройке) | #transfer-between-wallets-within-an-account |
| account.rs, all_balances.rs | балансы спота, «Проверить подключение», все счета | #query-wallets, #query-wallets-2, #query-asset |
| user_stream.rs, user_stream_conn.rs, wallet_update.rs | поток аккаунта wallet-order (баланс) | #subscribe-wallet-order |

## Что в документации НЕ определено (решения — настройками, проверить на живых ответах)

1. **Имя сети BSC** (`chainName`) — в примерах только BTC, ETH, TRX, EOS → `venue.bsc_network` по живому `chainCfg`.
2. **Поле статуса пары** в `/public/products` не описано → `venue.pair_status_listed` (значение из живого ответа).
3. **Имена полей пар** (`baseCurrency`, `quoteCurrency`, `priceScale`, `…TickSizeEv`, `…OrderValueEv`) — из таблиц старой
   документации и полей ордеров; сверить с записью.
4. **Номер `sequence` стакана** — идут ли подряд, не сказано → по умолчанию проверяется рост; `venue.book_seq_strict` —
   строго +1. Разрыв — пара недостоверна до снимка (снимок приходит сам раз в 60 с; отписки одной пары нет).
5. **«5 соединений на клиента»** — что такое клиент, не сказано → `venue.ws_max_conns`; потолок ~100 стаканов.
6. **Формат `clOrdID`** — в примерах UUID → свой ID — случайный UUID v4 (проверить первым ордером).
7. **Итог ордера** `cumBaseValueEv` / `cumQuoteValueEv` — смысл полей не описан (исполнено / сумма) — первым ордером.
8. **Комиссия вывода** — внутри суммы или сверху, не сказано; **точность суммы вывода** не описана (None).
9. **Счёт зачисления депозита** (SPOT / Contract) — у адреса нет параметра → `venue.deposit_to_spot` после первого депозита.
10. **Статусы вывода** `Success` (таблица) / `Succeed` (пример) — принимаются оба.
11. **fee-rate** — параметр `quoteCurrency` (строка) или `currency` (таблица), страницы (limit 20?) — проверить.
12. **Заголовок `x-ratelimit-retry-after-<group>`** транспорт шлюза не читает → пауза `venue.rate_limit_pause_ms`.
13. **Права ключа** — запроса о ключе нет; отдельного права «только спот» нет (деньги на фьючерсах не держать).
14. **Поток аккаунта** — в сообщении весь баланс, без причины изменения: вид (депозит / вывод / ордер) выводится из
    наличия событий ордеров в том же сообщении — проверить на живой записи до `venue.balance_events = да`.
15. Скидки комиссии своей монетой у Phemex в документации нет (урок 23 — обычная ставка, монета комиссии — по факту).
16. Earn — только поток `wm.subscribe` (REST нет), фьючерсы coin-M — масштаб из справочника контрактов (не читается).
17. **Сторож пар (решения 202/213, 08.11)**: `feed_health` — scheduled = false, per_pair = true, confirm = true — по
    содержимому: sequence ответа GET /md/orderbook и sequence кадра документация не связывает. Отписки одной пары нет —
    «переподписать пару» = orderbook.unsubscribe + orderbook.subscribe всех пар соединения (как у CoinEx), соседи
    получают новый снимок. Контрольный снимок раз в 60 с — тишина пары дольше 60 с с запасом означает потерю.

## Ключ и кабинет

Ключ «Read & Trade & Withdraw» **с обязательной привязкой IP** сервера; вывод — только с главного аккаунта и только на
адреса из «withdrawal address list» кабинета Phemex (кошелёк бота BSC, адреса других бирж) — без этого вывод через API
невозможен. Секреты — `phemex.api_key`, `phemex.api_secret` (docs/SECURITY.md).

## Не подключено

Окна (window_tap — запись сделок), пробный ордер (его нет у Phemex), VIP-адрес `vapi.phemex.com` (нужен белый список
IP у Phemex — только настройкой `venue.rest_url` / `venue.ws_url`).
