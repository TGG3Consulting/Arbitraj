# Bybit (API V5, только спот) — интеграция и чек-лист уроков BingX/MEXC

Решение 136, ветка `router/bybit` от `feat/own-bsc-node` (85a7dcf). Адаптер — `src/venues/bybit/` (README.md папки —
файлы и источники). Документация — исходник официальной документации `github.com/bybit-exchange/docs` (коммит 76aa20a,
2026-11-03; страницы https://bybit-exchange.github.io/docs/v5/…), сверка snapshot/delta — официальный SDK pybit
(коммит c3309e1). Живые публичные ответы 04.11 — `tests/data/cex_samples_2026_11_04/bybit_*` (тесты
`tests/bybit_real_data.rs`).

## Что сделано (контракт целиком)

| Функция контракта | Как у Bybit |
| --- | --- |
| Справочник пар | GET /v5/market/instruments-info?category=spot (публичный, без пагинации): basePrecision — шаг количества, tickSize — шаг цены, minOrderAmt, maxLimitOrderQty, status (Trading). Признака «API» нет — api_buy/api_sell = Trading |
| Монеты и сети | GET /v5/asset/coin/query-info (весь — фоном вне потоков tokio, кэш venue.networks_fresh_ms; одна монета — ?coin=, решение 113) |
| Стакан | WS orderbook.{1,50,200,1000}.{SYM}: snapshot + delta, локальный стакан, проверка роста u, разрыв → пересподписка пары; ≤10 args на запрос, ≤100 на соединение, ≤21000 символов; {"op":"ping"} |
| Котировка | По уровням стакана + taker из /v5/account/fee-rate (фоном, 5/s) |
| Ордер | Limit + timeInForce IOC/FOK, isLeverage=0, orderLinkId ≤36 на попытку; итог — поток order.spot или /v5/order/realtime → /history; не дождались — отмена; комиссия — cumFeeDetail или execution (сверка execQty) |
| check_order | order_rules::ready (пробного ордера у спота нет); предел цены /v5/market/price-limit при venue.price_limit_check |
| Вывод, статус | /v5/asset/withdraw/create (forceChain=1, feeType=1, requestId, accountType = venue.withdraw_account), /withdraw/query-record |
| Адрес депозита, статус | /v5/asset/deposit/query-address (tag — отказ), /deposit/query-record?txID= |
| Перевод на торговый счёт | /v5/asset/transfer/inter-transfer FUND → UNIFIED (venue.deposit_to_spot) |
| Балансы | /v5/account/wallet-balance?accountType=UNIFIED (и &coin= для одной монеты) |
| Проверить подключение | /v5/user/query-api + /v5/account/info: Read-Write, master, uta=1, права есть, прав деривативов нет, unifiedMarginStatus 3–6, IP |
| Окна 125 | кадры стакана торговых соединений; свои соединения publicTrade.{SYM} и orderbook.{venue.window_depth_limit}.{SYM} |
| Маппинг кошельков (132) | Строка bybit появляется сама — экран строит строки по всем CEX реестра; новых настроек нет |

## 20 уроков → как закрыты

| # | Урок | Как закрыт (файл:строка) |
| --- | --- | --- |
| 1 | Только документация, ссылка в функции | Шапки всех файлов `src/venues/bybit/*.rs` со ссылкой на раздел; неясное — вопросы в docs/TASKS.md (Bybit) |
| 2 | Противоречия — на живом ответе | ts стакана: в таблице number, в примечании строка — читаются оба (depth.rs `int_of`); живая запись 04.11 — число. Кадр publicTrade ломал типизированный разбор стакана — найдено на живой записи, исправлено (depth.rs, `Head`) |
| 3 | Тесты на записях | tests/bybit_real_data.rs: 5 тестов на живых публичных ответах 04.11; приватные — 4 #[ignore] с описанием записи |
| 4 | Одна функция правил | order_rules.rs:97 `ready` — её зовут trade.rs и mod.rs:152 `check_order`; шаги, minOrderAmt, maxLimitOrderQty (order_rules.rs:53); фильтр цены — price-limit (order_rules.rs:137); пробного ордера у спота нет (Pre Check Order: «inverse, linear, option») |
| 5 | IOC | order_rules.rs:37 — только IOC/FOK (enum timeInForce); маркет-ордер не предлагается. IOC у спота Limit — подтвердить первым живым ордером (вопрос) |
| 6 | Свой ID на попытку | order_rules.rs:165 `link_id` — метка мс + сделка, ≤36 (order_rules.rs:35), алфавит по документации; вывод — requestId ≤32 (transfers.rs) |
| 7 | Факт исполнения и комиссии | trade.rs `result` (cumExecQty, cumExecValue, Rejected → отказ); fills.rs:49 — cumFeeDetail или исполнения со сверкой суммы execQty; сроки и limit (1..100, fills.rs:35) проверяются в ready до отправки |
| 8 | По адресу контракта; имя сети BSC — настройкой | networks_parse.rs:71 — адрес только у сети venue.bsc_network; имя сети — из поля chain (записать на живом) |
| 9 | Большой справочник | wallet.rs:79 get_heavy + :81 spawn_blocking, кэш за Arc; одна монета — wallet.rs:58; SpotPairs не толкается без изменений (pairs_watch.rs, как MEXC) |
| 10 | Ввод/вывод, memo, FUND | networks_parse.rs: статусы, комиссия, минимумы, precision (:97), подтверждения (:96); tag адреса — отказ (deposit_addr.rs:68); FUND → UNIFIED — fund_move.rs:34 |
| 11 | Комиссия по паре из API, монета по факту | fees.rs (fee-rate по символу); монета — cumFeeDetail / feeCurrency (fills.rs) |
| 12 | Стакан, пределы, пинг, без бесконечных ресинков | local_book.rs:63 (снимок/изменение, :79 рост u), ws_conn.rs:149 разрыв → пересподписка, :152 повтор раньше venue.ws_reconnect_pause_ms → переподключение; ws_conn.rs:39 ≤10 args, market.rs:29 ≤100, market.rs:120 split ≤21000 символов; ping ws_conn.rs:90 |
| 13 | Минимум выделений | depth.rs — serde со ссылками на текст кадра; ключ пары создаётся один раз (ws_conn.rs:126); стакан собирается раз за кадр, одна копия ядру |
| 14 | Пересподписка не перезапускает круги | subs.rs:121–124 — fee-rate при том же списке не перезапускается (как MEXC) |
| 15 | REST-лимиты, без REST на горячем пути | reply.rs:30 (403 → 10 мин), :87 (X-Bapi-Limit-Status = 0) и :102 (10006) — эндпоинт молчит до сброса; gate перед каждым запросом (reply.rs:45). Котировка — только память (book.rs) |
| 16 | Приватный WS | user_stream.rs / user_stream_conn.rs — order.spot, execution.spot, wallet (auth по документации); итог ордера по событию (private_cache.rs). Изменения баланса ядру неприменимы: депозит на FUND, wallet — только UNIFIED без причины (user_stream.rs:7) |
| 17 | Пять частей | Код; настройки в каталоге (4 новых); UI — площадка из реестра, «Проверить подключение», настройки области bybit, секреты; комментарии; ARCHITECTURE, SETTINGS, DECISIONS 136, FIRST_RUN |
| 18 | Секреты | bybit.api_key / bybit.api_secret — только в зашифрованном файле (secrets/mod.rs KNOWN); в журнал не пишутся; подпись WS — в памяти |
| 19 | Права ключа | account.rs:31 REQUIRED (SpotTrade, AccountTransfer, Withdraw), :37 FORBIDDEN (ContractTrade, Options, Derivatives — отказ проверки), master (:90), UTA; вывод — только на адреса из адресной книги Bybit («make sure you add address in the address book first») |
| 20 | Ядро и шлюз не менялись | Изменены только main.rs (регистрация), venues/mod.rs, secrets KNOWN, каталог настроек (4 строки + описание), документы. Кошелёк — строка «Маппинга кошельков» сама |

## Что осталось проверить на живом (замер нагрузки — решение 109, ADAPTER_TEMPLATE п. 8)

См. docs/TASKS.md («Bybit — после выкладки»). Замер CPU/памяти с включённой Bybit при режиме «Стоп» — не сделан.
