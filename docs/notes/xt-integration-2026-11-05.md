# XT.com — интеграция (решение 167): как закрыт каждый урок

Claude, 05.11.2026. Адаптер `src/venues/xt/` на спот API v4. Чек-лист — `docs/notes/cex-integration-lessons-2026-11-04.md`
(30 пунктов), анализ — `docs/notes/xt-analysis-2026-11-05.md`. Документация прочитана по исходнику doc.xt.com
(github.com/xt-com/xt4-api, коммит `8edf738`). Живых ответов нет (из среды разработки sapi.xt.com закрыт прокси): всё,
что помечено «вживую», — проверить на сервере бота по `tests/README.md`, раздел XT.

| # | Как закрыт или почему неприменим |
|---|---|
| 1 | Ссылка на раздел документации (`doc.xt.com/#…` и файл исходника) — в шапке каждого файла; чего нет в документации — README адаптера, «Проверить на живых ответах» |
| 2 | Противоречия записаны, не угаданы: заголовки `validate-*` (документация) против `xt-validate-*` и hex заглавными (SDK pyxt) — следуем документации, `sign.rs`; recvWindow 5000 / 60000 — настройка, предел 60000 (`rest.rs`); returnCode фьючерсов 0 / 200 — оба успех (`reply.rs`); type/clientOrderId вывода числом — поле type не читается (`transfers.rs`); timeInForces пары — сверяется, но проверить живым ордером (урок 30) |
| 3 | Тесты `tests/xt_real_data.rs`: сборка строки подписи на примере документации и HMAC на RFC 4231 (всегда); разбор ответов — `#[ignore]` до записи `xt_*`; команды записи — `tests/README.md` |
| 4 | Одна функция `order_rules::ready` — у `trade.rs` и у `check_order` (`mod.rs`): ONLINE + tradingEnabled + openapiEnabled, LIMIT в orderTypes, IOC/FOK в timeInForces, шаг от min (`range.rs`), PRICE/QUANTITY/QUOTE_QTY; пробного ордера нет — `venue.order_test_first` = да даёт ошибку настройки; PROTECTION_LIMIT/ONLINE (от последней цены сделки и цены открытия) не проверяются — отказ ORDER_F04xx/F05xx до движения денег |
| 5 | timeInForce IOC или FOK (`order_rules.rs`, TIME_IN_FORCE); не дождались итога — отмена `DELETE /v4/order/{id}` (`order_state.rs::settle`) |
| 6 | clientOrderId «^[a-zA-Z0-9_]{4,32}$», метка попытки — новый на каждую попытку (`order_rules.rs::client_id`), у вывода — так же (`transfers.rs`) |
| 7 | POST отдаёт только orderId → итог GET /v4/order/{id}: executedQty, avgPrice, комиссия из deductServices (оплата в XT) или fee/feeCurrency; разные монеты — ошибка для ручной сверки (`order_state.rs::fee`); сроки проверяются в `ready` до отправки |
| 8 | Сопоставление по `contract` сети BSC (`networks_parse.rs`); название сети — `venue.bsc_network` (в документации не названо) |
| 9 | Справочники сетей и пар — только целиком: разбор в spawn_blocking, кэш за Arc (`wallet.rs`, `symbols.rs`); пары с `version` — без изменений список не приходит и не пересобирается (решение 131) |
| 10 | depositEnabled/withdrawEnabled по сети и depositStatus/withdrawStatus по монете (`networks_parse.rs`), withdrawFeeAmount, withdrawMinAmount; точности вывода в документации нет (`withdraw_precision` None); memo адреса — отказ (`deposit_addr.rs`); ненулевой depositFeeRate — ввод по сети закрыт (смысл не описан, вопрос Тиграну); счёт зачисления не описан — перевод `/v4/balance/transfer` не подключён до живого депозита |
| 11 | Ставки по ключу в API нет — takerFeeRate пары (`book.rs`, `symbols.rs::taker_fee`), монета комиссии — из ордера; вопрос Тиграну (VIP-уровень не виден) |
| 12 | `venue.book_stream`: `depth` (снимки 5/10/20/50, 1000 мс) или `depth_update` (100 мс) + снимок REST из общей очереди ≤ 10/с (`snapshots.rs`) по «Orderbook manage» (`local_book.rs`); разрыв — пара недостоверна, новый снимок не раньше venue.ws_reconnect_pause_ms (`ws_diff.rs`, без круга ресинков); текст «ping» < 60 с (`ws.rs`); лимиты подписок в документации нет — настройки, живой замер |
| 13 | Кадры разбираются с заимствованием строк (`depth.rs`); снимок с тем же updateId — без разбора уровней и без отдачи ядру (`ws_book.rs::on_depth`) |
| 14 | Фоновых кругов нет (ставка — в справочнике пар); пересподписка не трогает pairs_watch (`subs.rs`) |
| 15 | 429 и GATEWAY_* — пауза venue.rate_limit_pause_ms (Retry-After у XT нет), отдельно спот и прочие адреса (`reply.rs`); /v4/withdraw — 1 запрос/с (`transfers.rs`); на горячем пути REST нет |
| 16 | Поток аккаунта **не подключён**: тема `balance` присылает итоговый баланс (b, f) без причины — депозит не отличить от сделки (как OKX account), ложные зачисления опаснее опроса; зачисление — `deposit_status` (README адаптера, вопрос Тиграну) |
| 17 | Настройки — `settings/catalog/xt.rs` (venue.book_stream, venue.depth_snapshot_per_sec, venue.depth_snapshot_limit, venue.futures_coin_rest_url, venue.user_center_url), docs/SETTINGS.md, FIRST_RUN; площадка — в реестре (экран «Площадки»); «Проверить подключение» — `key_check.rs` |
| 18 | Секреты xt.api_key, xt.api_secret — `secrets/mod.rs`, docs/SECURITY.md; ответ user-center с secretKey не пишется в сырой журнал (`reply.rs`, Raw::Secret) |
| 19 | Права QUERY + TRADE + WITHDRAW, без FUTURE/LEVER_LOAN (SECURITY.md); roleScopes QUERY_NO_TRADE — отказ проверки; адресная книга (WITHDRAW_004) — в README и SECURITY |
| 20 | Ядро и шлюз не менялись: регистрация в main.rs, venues/mod.rs; тексты — notify/human.rs, reason.rs, reason_codes.rs (решение 164) |
| 21 | Минимума депозита в API нет → `venue.deposit_min_absent_ok` (`networks_parse.rs`) |
| 22 | «Предварительно зачислено» нет; T+1 (WITHDRAW_009) заранее не узнать → withdraw_hold по умолчанию пустой (`deposits.rs` шапка) |
| 23 | Скидка оплатой в XT: ставка API не сообщается — расчёт по обычной ставке, факт — deductServices (`book.rs`, `order_state.rs`) |
| 24 | account_balances: Спот, Фьючерсы USDT-M и Coin-M (своя подпись, `futures.rs`), Маржа — примечание (только по паре); Earn в API нет (`all_balances.rs`) |
| 25 | Адрес депозита — кэш (монета, сеть); DEPOSIT_006/007 «создаётся» — InvalidAddress с пояснением (`deposit_addr.rs`) |
| 26 | Фоновых ставок нет — неприменимо |
| 27 | «Фьючерсный счёт не открыт» — код в документации не показан: пока ошибка строки счёта (вживую) |
| 28 | gzip — reqwest (общий клиент); IPv4/IPv6 в документации нет |
| 29 | Своя запись ключа — GET /v4/user/account/api-key на venue.user_center_url (isLock, roleScopes, bindIps); имена прав — из createAccountApiKey.md; WITHDRAW в ответе не виден — проверяется первым выводом |
| 30 | IOC: есть в timeInForces примера; итоговое state IOC с остатком (CANCELED/EXPIRED) — оба окончательные, проверить живым ордером |

Отдельно: tokio-tungstenite 0.30 не поддерживает permessage-deflate, а документация требует заголовок — подключение
без него; принимает ли сервер — первая проверка на сервере бота (`websocat` без сжатия, tests/README.md).
