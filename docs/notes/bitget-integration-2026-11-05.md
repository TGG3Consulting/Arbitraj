# Bitget — интеграция (решение 162): как закрыт каждый урок

Claude, 05.11.2026. Адаптер `src/venues/bitget/` на Classic API v2. Чек-лист — `docs/notes/cex-integration-lessons-2026-11-04.md`
(30 пунктов), анализ — `docs/notes/bitget-analysis-2026-11-05.md`. Живых ответов нет (из среды разработки
api.bitget.com закрыт прокси): всё, что помечено «вживую», — проверить на сервере бота по `tests/README.md`, раздел Bitget.

| # | Как закрыт (файл:строка) или почему неприменим |
|---|---|
| 1 | Ссылка на раздел документации — в шапке каждого файла; ссылки только на `/docs/...` (старые `/api-doc/...` ведут на UTA). Чего нет в документации — раздел «Чего нет и почему» в `src/venues/bitget/README.md` |
| 2 | Противоречия записаны и обойдены без догадок: путь справочника монет (документация `/public/coins`, SDK `/market/coins`) — `wallet.rs` шапка; «cancelled»/«canceled» — оба окончательные, `order_state.rs` (FINAL); data all-trade-rate — схема «объект», пример «массив», `fees.rs:parse_all`; время входа WS (мс / с) — поток аккаунта не подключён; поле chain адреса депозита («BTC-Bitcoin» vs «BTC») не сверяется, `deposit_addr.rs` |
| 3 | Тесты: подпись — вектор функций SDK (`tests/bitget_real_data.rs`, идут всегда); разбор ответов — `#[ignore]` до записи `tests/data/cex_samples_2026_11_05/bitget_*`; команды записи — `tests/README.md` |
| 4 | Одна функция `order_rules::ready` (`order_rules.rs:98`) — у `trade.rs` и у `check_order` (`mod.rs`); шаги вниз/по стороне, minTradeUSDT, status online; пробного ордера нет — `venue.order_test_first` = да даёт отказ (`order_rules.rs:100`); фильтр цены buy/sellLimitPriceRatio не проверяем (база цены не определена) |
| 5 | `force` = ioc или fok (`order_rules.rs`, TIF_ALLOWED); не дождались итога — отмена остатка (`order_state.rs:61`) |
| 6 | Свой clientOid — новый на каждую попытку, ≤ 32, [A-Za-z0-9_-] (`order_rules.rs:134`), тест правила |
| 7 | Итог — orderInfo: baseVolume, quoteVolume, комиссия и её монета из feeDetail (`order_state.rs:128`, `order_fee.rs:30`); сроки venue.order_poll_ms / order_final_wait_ms проверяются в `ready` до отправки; окно истории выводов venue.history_window_s — до отправки вывода (`transfers.rs:49`) |
| 8 | Сопоставление по `chains[].contractAddress` сети BSC (`networks_parse.rs`); имя сети — `venue.bsc_network` из живого ответа (в документации только пример «BTC») |
| 9 | Весь справочник — разбор в `spawn_blocking`, кэш за Arc (`wallet.rs:98`); одна монета `?coin=` — `coin_networks_of` (`wallet.rs:60`, решение 113) |
| 10 | rechargeable / withdrawable, withdrawFee, minWithdrawAmount, withdrawMinScale → точность, withdrawStep → кратность (`networks_parse.rs:119`), depositConfirm; needTag и tag адреса — отказ (`networks_parse.rs:66`, `deposit_addr.rs`); extraWithdrawFee ≠ 0 — комиссия неизвестна. Счёт зачисления не описан — переводы spot ↔ p2p настройками (`fund_move.rs`) |
| 11 | Ставка по ключу и паре — all-trade-rate (`fees.rs`); монета комиссии — из ордера |
| 12 | books1/5/15 снимками (venue.depth_levels), ≤ 1000 каналов на соединение (совет < 50), ≤ 10 сообщений/с, подписка ≤ 4096 байт (`ws_conn.rs:40`), «ping» строкой < 2 мин (`ws.rs`) |
| 13 | Один разбор JSON на кадр (`depth.rs:53`); снимок с тем же seq ядру не отдаётся, без копий уровней (`ws_conn.rs:133`) |
| 14 | Круг ставок перезапускается только при новом списке символов (`subs.rs`); подписка — только при подключении (240 запросов/час/соединение) |
| 15 | 429 (HTTP или код) — пауза всех запросов на Retry-After / venue.rate_limit_pause_ms (`reply.rs:53`); на горячем пути REST нет — котировка из памяти |
| 16 | Приватный WS — **не подключён**: страница канала баланса спота Classic не прочитана, время входа противоречиво; вопрос Тиграну (README адаптера). Баланс — REST в предпроверке |
| 17 | Настройки — каталог (новая `venue.history_window_s`, `settings/catalog/bitget.rs`), docs/SETTINGS.md, FIRST_RUN; площадка на «Площадках» через реестр; «Проверить подключение» (`account.rs`) |
| 18 | Секреты bitget.api_key / api_secret / api_passphrase — `src/secrets/mod.rs`, docs/SECURITY.md |
| 19 | Нужны stow, wwow (+ wtow при переводах); лишние coow, cpow, smow, ttow — отказ (`account.rs:36`); белый список IP обязателен для вывода (`account.rs:143`); адресная книга (код 11000) — в тексте проверки и SECURITY |
| 20 | Ядро и шлюз не менялись: регистрация в main.rs, venues/mod.rs |
| 21 | minDepositAmount прямо в справочнике; «0»/пусто → None, при venue.deposit_min_absent_ok = да — 0 (`networks_parse.rs:99`) |
| 22 | Статуса «предварительно зачислено» нет в документации → withdraw_hold по умолчанию (пусто), `deposits.rs` шапка |
| 23 | BGB: расчёт всегда по обычной ставке (худший случай), состояние deduct-info — в журнал и в «Проверить подключение» (`fees.rs:127`); факт d (BGB) / t (монета) — `order_fee.rs`; размер скидки не описан — вопрос |
| 24 | account_balances: Спот, Фонд, Боты (спот, фьючерсы), итог всех счетов в USDT (`all_balances.rs:43`); сбой счёта — в его error |
| 25 | Адрес депозита — кэш (монета, сеть), запрос в предпроверке через контракт (`deposit_addr.rs`) |
| 26 | Сбой круга ставок → повтор через venue.fee_retry_s (`fees.rs:121`) |
| 27 | Кодов «счёт не открыт» в документации нет: ошибка счёта — в его error панели (проверить вживую) |
| 28 | gzip — общий HTTP-клиент (reqwest gzip, решение 142); размер полного справочника и IPv4/IPv6 — замерить вживую |
| 29 | Права — коды authorities из официального перечня Get Account Information (`account.rs`) |
| 30 | IOC в справочнике пар не перечисляется вовсе (флагов типов нет), force=ioc есть в Place Order — проверить первым живым ордером |

## Вопросы Тиграну (для docs/TASKS.md — допишет координатор)

1. Счёт Bitget бота — классический (не UTA)? Что отвечает upgrade-status классическому счёту — проверить.
2. Куда зачислять депозит: Modify Deposit Account → spot по монетам или venue.deposit_to_spot = да? С какого счёта
   выводит Bitget и входит ли комиссия в size — проверить малым выводом.
3. Скидка BGB: включать ли (deduct on)? Размер скидки в документации нет — расчёт остаётся по обычной ставке.
4. Поток аккаунта: нужна страница канала баланса спота (Classic → WebSocket → Spot → Private → Account) и живой вход.
5. venue.history_window_s, venue.rate_limit_pause_ms, venue.ws_silence_ms — значения (методы — FIRST_RUN).
