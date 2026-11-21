# Poloniex — интеграция (решение 169): как закрыт каждый урок

Claude, 05.11.2026. Адаптер `src/venues/poloniex/` на Spot API. Чек-лист — `docs/notes/cex-integration-lessons-2026-11-04.md`
(30 пунктов), анализ — `docs/notes/poloniex-analysis-2026-11-05.md`. Живых ответов нет (из среды разработки
api.poloniex.com закрыт прокси; документация api-docs.poloniex.com прочитана пересказчиком страниц, SDK — с GitHub
напрямую): всё, что помечено «вживую», — проверить на сервере бота по `tests/README.md`, раздел Poloniex.

| # | Как закрыт (файл:строка) или почему неприменим |
|---|---|
| 1 | Ссылка на раздел документации — в шапке каждого файла и у функций; чего нет в документации — README.md адаптера, раздел «Чего нет» |
| 2 | Противоречия обойдены без догадок: clientOrderId 64 / «17 digits» → свой id 17 цифр, проходит оба (`order_rules.rs:173`); статусы депозита COMPLETED/COMPLETE и вывода PROCESSING/PENDING — принимаются оба (`deposits.rs`, `transfers.rs`), неизвестный — ошибка с сырым ответом; signatureVersion 1/2 — не шлётся (как SDK, `sign.rs`); `network` вывода — шлётся всегда |
| 3 | Тесты: подпись REST (GET/POST/DELETE) и входа в WS — вектор функций SDK (`tests/poloniex_real_data.rs`, идут всегда); разбор ответов — `#[ignore]` до записи `tests/data/cex_samples_2026_11_05/poloniex_*` (команды — `tests/README.md`). Моков нет |
| 4 | Одна функция `order_rules::ready` (`order_rules.rs:136`) — у `trade.rs` и у `check_order` (`mod.rs:149`); `check_rules` (`order_rules.rs:63`): state NORMAL, шаги (количество вниз, цена по стороне), min/maxQuantity, min/maxAmount, > 1 USDT (21350), highestBid / lowestAsk; пробного ордера нет — `venue.order_test_first` = да даёт отказ |
| 5 | timeInForce = IOC или FOK (`order_rules.rs`, TIF_ALLOWED); не дождались итога — отмена остатка (`order_state.rs:68`) |
| 6 | Свой id — новый на каждую попытку (мс + номер, `order_rules.rs:173`), тест; у вывода — тот же вид в withdrawOrderId (идемпотентный ключ) |
| 7 | Итог: Order Details (filledQuantity, filledAmount) + комиссия из сделок ордера (`fills.rs:57`, ждёт, пока количество сделок не сойдётся с filled); сроки venue.order_poll_ms / order_final_wait_ms и окно venue.history_window_s проверяются ДО отправки ордера и вывода |
| 8 | Сопоставление по `contractAddress` сети BSC (`networks_parse.rs`); имя сети — `venue.bsc_network` по живому ответу (в документации нет) |
| 9 | Весь справочник /v2/currencies — разбор вне рабочих потоков, за `Arc` (`wallet.rs:103`); одна монета — /v2/currencies/{currency} (`wallet.rs`, coin_networks_of); пересподписка только при смене набора стаканов (`pairs_watch.rs`) |
| 10 | Ввод/вывод, комиссия, минимум вывода («≤ 0 — нет»), подтверждения — из v2; memo (address-payment-id) — в журнал и отказ адреса; delisted — ввод и вывод закрыты; точность вывода (`decimals`) — смысл не ясен, не используется; счёт один (SPOT) — внутреннего перевода нет (`mod.rs:191`) |
| 11 | Ставки /feeinfo одним запросом: specialFeeRates по паре, иначе общая (`fees.rs`); монета комиссии — feeCurrency сделок |
| 12 | Канал book снимками 5/10/20 (`ws.rs`, DEPTH_LEVELS); предела символов в документации нет — по одному в сообщении, `venue.ws_symbols_per_conn` — Тигран; ping {"event":"ping"} < 30 с (`ws.rs:108`); ошибка подписки без символа — запись в журнал, без бесконечных переподключений (`ws_conn.rs`) |
| 13 | Один разбор JSON на кадр, уровни переносятся в OrderBook без копий; повтор снимка с тем же id — только время (`ws_conn.rs:118`) |
| 14 | Круг ставок перезапускается только при новом списке символов (`subs.rs`, решение 131); поток аккаунта — один на адаптер |
| 15 | 429 → пауза всех запросов на Retry-After или `venue.rate_limit_pause_ms` (`reply.rs:52`, gate — `reply.rs:31`); ответ при превышении в документации не описан — записать вживую; на горячем пути REST нет (ставки и стаканы — из памяти) |
| 16 | Приватный WS: вход auth с подписью, канал balances (`user_stream.rs`, `account_update.rs`); изменения — разница остатков (опора — REST при входе, `account_update.rs:153,163`) |
| 17 | Код + настройки (существующие venue.*, описания дополнены «Poloniex (решение 169)» в каталоге и docs/SETTINGS.md) + площадка в интерфейсе из реестра («Проверить подключение», ключи) + комментарии; ARCHITECTURE — координатор |
| 18 | Секреты `poloniex.api_key`, `poloniex.api_secret` — только в зашифрованном файле (`src/secrets/mod.rs`); ключ вырезается из сырых кадров приватного WS (`user_stream.rs`) |
| 19 | Права через API не проверяемы (эндпоинта нет) — сказано в «Проверить подключение» и docs/SECURITY.md; белый список IP и адресов — кабинет |
| 20 | Ядро и шлюз не менялись: регистрация в `main.rs`, `venues/mod.rs`; имя и коды для человеческих текстов — `notify/human.rs`, `notify/reason.rs`, `notify/reason_codes.rs` (решение 164) |
| 21 | Минимума депозита в API нет → `venue.deposit_min_absent_ok` (да — 0, иначе None) (`networks_parse.rs`) |
| 22 | Статуса «предварительно зачислено» нет в документации → `withdraw_hold` по умолчанию (пусто); подтвердить живым депозитом |
| 23 | Скидка TRX (`trxDiscount`): расчёт по обычной ставке, изменение — в журнал (`fees.rs`), факт и монета — из сделок |
| 24 | `account_balances` (`all_balances.rs`): Спот, Маржа (оценка и займы), Фьючерсы — error «отдельные ключи»; funding/earn в Spot API нет |
| 25 | Адрес депозита — кэш по (монета, сеть), в предпроверке (`deposit_addr.rs`) |
| 26 | Сбой круга ставок — повтор через `venue.fee_retry_s` (`fees.rs:114`) |
| 27 | Аналога «счёт не открыт» в документации нет — ошибка маржи идёт в её error; проверить вживую |
| 28 | gzip — у общего HTTP-клиента; IPv4/IPv6 — в документации Poloniex не описано (venue.api_ip не нужен) |
| 29 | Списка прав ключа в API нет — проверить нечем (сказано в ответе кнопки) |
| 30 | IOC не перечисляется в справочнике пары (поля типов нет) — правила его не требуют; подтвердить первым живым ордером |

## Проверить на живых ответах (до первой сделки)

1. Имя сети BSC в /v2/currencies (`blockchain`) и `coin` сети — по нему /wallets/addresses отдаёт адрес BSC?
2. Статусы депозита и вывода; есть ли txid внутри status вывода; входит ли комиссия в amount вывода.
3. Принят ли clientOrderId из 17 цифр и IOC; feeCurrency покупки (базовая или TRX).
4. Тело ошибки REST (code, message), ответ 429, коды неверной подписи и ключа без права вывода.
5. Кадры balances: изменение по разнице остатков сходится с депозитом/выводом; приходит ли balances по deposit.
6. highestBid / lowestAsk у CAKE_USDT (бывает ли 0), minAmount; сколько символов держит одно соединение.
