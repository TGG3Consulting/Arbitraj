# CoinEx — интеграция (решение 170): как закрыт каждый урок

Claude, 05.11.2026. Адаптер `src/venues/coinex/` на API v2 (только спот). Чек-лист — `docs/notes/cex-integration-lessons-2026-11-04.md`
(30 пунктов), анализ — `docs/notes/coinex-analysis-2026-11-05.md`. Страницы документации docs.coinex.com перечитаны при
написании кода (ссылки — в шапках файлов). Живых ответов нет (из среды разработки api.coinex.com закрыт прокси): всё, что
помечено «вживую», — проверить на сервере бота по `tests/README.md`, раздел CoinEx.

| # | Как закрыт (файл:строка) или почему неприменим |
|---|---|
| 1 | Ссылка на раздел документации — в шапке каждого файла `src/venues/coinex/*`; чего нет в документации — раздел «Чего нет и почему» в `src/venues/coinex/README.md` и вопросы ниже |
| 2 | Расхождения записаны и обойдены без догадок: finish/finished депозита — оба (`deposits.rs`); «audit» вывода вне перечня — ожидание (`transfers.rs`); coin/ccy в примерах — по таблице; is_pre_trading_available / is_pre_market_… — не используется; confirmation/confirmations — не используется; last_fill_amount / last_filled_amount — не используется; строка/число/boolean у флагов и целых — оба (`num.rs`); checksum стакана (int32 против 2578768879) — не сверяется (`depth.rs` шапка); единицы taker_rate («0.03» против «0.002») — настройка `venue.fee_rate_percent` (`fees.rs`) |
| 3 | Тесты: строка подписи — пример документации, HMAC hex — RFC 4231 (`tests/coinex_real_data.rs`, идут всегда); разбор ответов — `#[ignore]` до записи `tests/data/cex_samples_2026_11_05/coinex_*`; команды — `tests/README.md` |
| 4 | Одна функция `order_rules::ready` (`order_rules.rs:100`) — у `trade.rs` и у `check_order` (`mod.rs`); шаги из precision (вниз количество, цена — по стороне), min_amount, status online + is_api_trading_available; пробного ордера нет — `venue.order_test_first` = да даёт отказ (`order_rules.rs:102`); фильтры 3606/3615/3618 заранее не проверяемы |
| 5 | Срок задаётся типом ордера: `type` = ioc или fok (`order_rules.rs:38`); не дождались итога — отмена остатка по order_id (`order_state.rs:61`) |
| 6 | Свой client_id — новый на каждую попытку, ≤ 32 байт, [A-Za-z0-9_-] (`order_rules.rs:137`), тест правила |
| 7 | Итог — order-status (filled_amount, filled_value, base/quote/discount_fee); ордер IOC без исполнения не хранится → сделки пары с фильтром order_id (`order_state.rs:69`, `deals.rs`): нет сделок и в ответе постановки filled_amount = 0 — «не исполнился» (`order_state.rs:70`), иначе «итог неизвестен». Сроки и venue.my_trades_limit проверяются в `ready` до отправки; venue.history_page_limit — до отправки вывода (`transfers.rs:49`) |
| 8 | Адрес контракта — `/v2/assets/info` chain_info.identity у цепи `venue.bsc_contract_chain` (`contracts.rs`, `wallet.rs:82`); сеть ввода/вывода — `venue.bsc_network`. Оба имени — по живым ответам (в документации нет; по примерам они разные). Адрес из ссылки обозревателя не разбирается. Без адреса — монета не сопоставляется (журнал) |
| 9 | Весь справочник и /assets/info — разбор в `spawn_blocking`, кэш за Arc (`wallet.rs:119`); одна монета — `?ccy=` (`wallet.rs:61`, решение 113) |
| 10 | deposit/withdraw_enabled монеты И сети, withdrawal_fee, min_withdraw_amount, withdrawal_precision, safe_confirmations; memo у BSC — журнал и отказ адреса депозита (`networks_parse.rs:122`, `deposit_addr.rs`); deflation_rate ≠ 0 — комиссия неизвестна (`networks_parse.rs:148`). Счёт зачисления — спот (funding-счёта у CoinEx нет), переводов не нужно — проверить первым депозитом |
| 11 | Ставка ключа по паре — trade-fee-rate (`fees.rs`); монета комиссии — из ордера, при скидке — fee_ccy сделок (`order_fee.rs:31`, `order_result.rs:56`) |
| 12 | Снимки `if_full = true`, limit 5/10/20/50 (`ws.rs`), один запрос depth.subscribe на соединение (`ws_conn.rs:71`) — что делает повторный запрос, не описано; лимиты подписок/соединений не описаны → `venue.ws_symbols_per_conn` без предела в коде; `server.ping` раз в `venue.ws_ping_ms` (интервал не описан) |
| 13 | Распаковка — один `Inflater` на соединение, буфер переиспользуется, текст без копии (`inflate.rs:61`, `ws_conn.rs:103`); один разбор JSON на кадр; снимок с тем же updated_at ядру не отдаётся (`ws_conn.rs:139`). Замер нагрузки (решение 109) — обязателен при первом запуске |
| 14 | Круг ставок перезапускается только при новом списке рынков (`subs.rs`) |
| 15 | 4213 / 3008 / 4001 / HTTP 429 — пауза всех запросов на Retry-After / venue.rate_limit_pause_ms (`reply.rs:53`, `reply.rs:92`); заголовки X-RateLimit-Remaining транспорт шлюза не отдаёт; на горячем пути REST нет — котировка из памяти |
| 16 | Приватный WS — **не подключён**: balance.update без причины изменения (отличить депозит от своего ордера нельзя без догадок). Зачисление — deposit_status (REST); баланс — REST в предпроверке |
| 17 | Настройки — каталог (новые `venue.bsc_contract_chain`, `venue.fee_rate_percent`, `venue.history_page_limit` — `settings/catalog/coinex.rs`), docs/SETTINGS.md, FIRST_RUN; площадка на «Площадках» через реестр; «Проверить подключение» (`account.rs`) |
| 18 | Секреты coinex.api_key / coinex.api_secret — `src/secrets/mod.rs`, docs/SECURITY.md; секрет уходит только подписью |
| 19 | Права ключа основного аккаунта API не отдаёт (только субаккаунты) — сказано в тексте «Подключено» и в SECURITY; белый список адресов вывода — обязателен (Submit Withdrawal), описан там же |
| 20 | Ядро и шлюз не менялись: регистрация в main.rs, venues/mod.rs; тексты — notify/human.rs (имя CoinEx), notify/reason.rs + reason_codes.rs (коды CoinEx, решение 164) |
| 21 | min_deposit_amount прямо в справочнике; «0»/пусто → None, при venue.deposit_min_absent_ok = да — 0 (`networks_parse.rs:150`) |
| 22 | Статуса «предварительно зачислено» нет → withdraw_hold по умолчанию (пусто); deposit_delay_minutes ≠ 0 у BSC — журнал (вопрос) |
| 23 | Скидка CET: состояние не читается, размер не описан — расчёт по обычной ставке (худший случай), факт — discount_fee + fee_ccy сделок |
| 24 | account_balances: Спот, Маржа (по рынкам), Фьючерсы, Финансовый (`all_balances.rs:34`); сбой счёта — в его error |
| 25 | Адрес депозита — кэш (монета, сеть), запрос в предпроверке через контракт (`deposit_addr.rs`); смена адреса в кабинете — до перезапуска (вопрос) |
| 26 | Сбой круга ставок → повтор через venue.fee_retry_s (`fees.rs:109`) |
| 27 | Кода «счёт не открыт» в документации нет: ошибка счёта — в его error панели (проверить вживую) |
| 28 | gzip — общий HTTP-клиент (решение 142); размер полных справочников и IPv4/IPv6 — замерить вживую (команды в tests/README.md) |
| 29 | Запроса прав своего ключа нет (поля trade_enabled / withdraw_enabled — только у субаккаунтов) — не выдумываются |
| 30 | IOC — значение order_type (в справочнике пары типов нет) — проверить первым живым ордером на минимальную сумму |

## Вопросы Тиграну (для docs/TASKS.md — допишет координатор)

1. Ордер IOC без исполнения CoinEx не сохраняет: итог «не исполнился» берётся по факту «сделок с этим order_id нет» и
   filled_amount = 0 в ответе постановки — так принимаем? Иначе каждый неисполненный IOC — «итог неизвестен».
2. Права ключа основного аккаунта API не отдаёт — хватит текста в «Проверить подключение» или работать ключом субаккаунта
   (его права читаются, /account/subs/api-detail)?
3. `deposit_delay_minutes` — считать ли удержанием вывода (withdraw_hold)? Смысл в документации не описан.
4. Скидку CET включать? Состояние не читается, расчёт остаётся по обычной ставке.
5. Поток аккаунта: balance.update без причины изменения — оставляем зачисление опросом deposit_status?
6. Значения `venue.bsc_network`, `venue.bsc_contract_chain`, `venue.fee_rate_percent`, `venue.ws_ping_ms`,
   `venue.ws_symbols_per_conn`, `venue.my_trades_limit`, `venue.history_page_limit` — по живым ответам (методы — FIRST_RUN).
7. Срок кэша адреса депозита (адрес можно сменить в кабинете) — нужен?
