# OKX — интеграция (решение 161): как закрыт каждый урок

Claude, 05.11.2026. Адаптер `src/venues/okx/` (README там же), анализ — `okx-analysis-2026-11-05.md`, чек-лист —
`cex-integration-lessons-2026-11-04.md`. Документация https://www.okx.com/docs-v5/en/ прочитана пересказчиком
(www.okx.com из среды закрыт прокси) — живых ответов нет; что записать — `tests/README.md`, раздел OKX.

| # | Урок | Как закрыт (файл) |
|---|---|---|
| 1 | Только документация, ссылка у функции | Шапки всех файлов `okx/*.rs` — раздел документации и цитаты |
| 2 | Расхождения — по живому ответу | Список «проверить на живом ответе» ниже и в tests/README.md |
| 3 | Тесты на записях | `tests/okx_real_data.rs`: подпись — без сети; разбор — `#[ignore]` до записи |
| 4 | Одна функция правил | `order_rules::ready` — у `trade.rs` и `check_order` (mod.rs) |
| 5 | IOC | `ordType` ioc / fok (`venue.order_type`), иное — отказ до отправки |
| 6 | Свой id на попытку | `order_rules::cl_ord_id` (мс + сделка, ≤ 32, буквы и цифры); вывод — `clientId` (transfers.rs); перевод — 128 бит (fund_move.rs) |
| 7 | Итог ордера, сроки до ордера | `order_state.rs`: accFillSz, avgPx, fee (−fee = списано), feeCcy; `times()` — до постановки; отменённые хранятся 2 ч — итог сразу |
| 8 | Сеть и адрес контракта | `networks_parse::core_network` (chain без «<ccy>-»), `venue.bsc_network`; ctAddr — только из currencies (у адреса депозита — 6 знаков); неполный — монета не сопоставляется, журнал |
| 9 | Большой справочник | Весь currencies — `heavy` + `spawn_blocking`, кэш `Arc`; одна монета — `ccy=` (wallet::one) |
| 10 | Ввод/вывод, memo, счёт зачисления | canDep/canWd, fee, minWd, maxWd, wdTickSz, needTag (журнал), адрес с tag/memo/pmtId — отказ; Funding ↔ Trading — fund_move.rs |
| 11 | Комиссия по ключу | trade-fee по паре, ставка = −taker (fees.rs); монета — feeCcy ордера |
| 12 | Стакан по документации | books5 (снимки) или books (local_book.rs: prevSeqId = прежний seqId, разрыв → пересподписка пары, повтор → переподключение); пинг «ping» < 30 с |
| 13 | Мало выделений | Один разбор JSON на кадр (depth.rs), уровни — один Vec на сторону; books — BTreeMap |
| 14 | Пересподписка не рвёт круги | subs.rs (как KuCoin): круг комиссий перезапускается только при новом списке |
| 15 | Лимиты, 429 | reply.rs: 50011/50013/50040/50061/58102, HTTP 429 — пауза пути (`venue.rate_limit_pause_ms`, Retry-After если есть); подключений ≤ 3/с (`venue.ws_connects_per_sec`), ≤ 30 на канал |
| 16 | Приватный поток | deposit-info на business (user_stream*.rs, deposit_event.rs) → MarketEvent::Balance с txId; account/balance_and_position причину «депозит» не дают — не слушаются |
| 17 | Готовая фича | Настройки (catalog/okx.rs, base.rs), «Проверить подключение» (account.rs), README, SETTINGS, FIRST_RUN |
| 18 | Секреты | okx.api_key / api_secret / api_passphrase (secrets/mod.rs, SECURITY.md); вход WS в журнал не пишется |
| 19 | Права, белый список | account.rs: trade + withdraw; вывод — только проверенные адреса (58207 → InvalidAddress с причиной) |
| 20 | Ядро и шлюз не меняются | Только регистрация в main.rs, секреты, каталог настроек |
| 21 | Минимум депозита | minDep прямо в currencies; нет — `venue.deposit_min_absent_ok` |
| 22 | Удержание вывода | deposits::withdraw_hold — депозиты state 1 «deposit credited» |
| 23 | Скидка своей монетой | Неприменим: «doesn't offer … offset trading fees by holding OKB» (справка Trading Fee Rules FAQ) |
| 24 | Все счета | all_balances.rs: Trading, Funding, Simple Earn, оценка (asset-valuation) |
| 25 | Адрес депозита — кэш, предпроверка | deposit_addr.rs (кэш по монете и сети) |
| 26 | Повтор ставок | fees.rs — `venue.fee_retry_s` |
| 27 | «Счёт не открыт» → пусто | Пустой data баланса — пустой счёт (account.rs, all_balances.rs) |
| 28 | gzip, IPv4/IPv6 | gzip — HTTP-клиент (reqwest gzip); `venue.api_ip` — сверка с ip ключа |
| 29 | Своя запись ключа | `/account/config`: perm, acctLv = 1, uid = mainUid, feeType = 0, ip |
| 30 | IOC, которого нет в справочнике | Перечня типов у пары OKX нет; ioc — штатный ordType; первый живой ордер малой суммы |

## Проверить на живом ответе

1. chain сети BSC (`CAKE-BSC`?) → `venue.bsc_network`; полный ли `ctAddr` в currencies; `canDep`/`canWd` — boolean.
2. Пустой `minDep` и его смысл → `venue.deposit_min_absent_ok`.
3. `to` у адреса депозита («6»/«18») и можно ли выбрать Trading для сущности счёта.
4. Знак `taker` у trade-fee (ожидается отрицательный).
5. `ruleType` у спот-пар (пустой считается normal).
6. books: порядок уровней, пустой кадр (seqId = prevSeqId), количество 0 в изменениях; сколько каналов на соединение
   выдерживает (4008/4009) → `venue.ws_symbols_per_conn`.
7. perm и право перевода Funding ↔ Trading (документация: «Transfer privilege», в perm такого значения нет).
8. 51020 (минимальная сумма) на CAKE-USDT; feeCcy у покупки и продажи при feeType 0.
9. deposit-info: state 1 и 2 по одному депозиту, subAcct = "" у основного счёта, pTime.
10. Состояния вывода по шагам; 58237 (rcvrInfo) для сущности счёта.

## Вопросы Тиграну

1. Сущность счёта OKX и домен (openapi / eea / us) → `venue.rest_url`, `venue.ws_url`, `venue.ws_business_url`.
2. Режим счёта Spot (acctLv 1) — «Проверить подключение» другой не пропустит.
3. Адрес депозита BSC — с зачислением на Trading (18), если сущность позволяет; иначе `venue.deposit_to_spot = да`.
4. Адреса вывода (кошельки DEX из «Маппинга», депозиты других бирж по BSC) — внести в кабинет OKX как проверенные.
5. Нужен ли rcvrInfo (Travel Rule) для выводов с этой сущности — сейчас не отправляется.
6. Запись окна (решение 125) для OKX — нужна ли (сейчас NotApplicable).
