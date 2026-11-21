# Gate (спот, API v4) — интеграция и чек-лист уроков BingX/MEXC (решение 135)

Ветка `router/gate` от `feat/own-bsc-node` (85a7dcf). Адаптер — `src/venues/gate/` (README.md папки — источники и файлы).

## Источники (откуда что взято)

- Основная документация https://www.gate.com/docs/developers/apiv4/en/ — прочитана через WebFetch (разделы Access URL,
  Authentication, Frequency limit rule, Error label list, API key permissions). Прямой curl к gate.com из песочницы закрыт
  прокси (403).
- Spot WebSocket v4 https://www.gate.com/docs/developers/apiv4/ws/en/ — WebFetch (формат запросов, spot.ping/pong,
  spot.order_book, spot.order_book_update, spot.obu, spot.trades, spot.balances, spot.orders, подпись).
- Официальный SDK https://github.com/gate/gateapi-python v4.106.179 (git clone): docs/*.md — поля моделей и параметры
  методов (сгенерированы из официальной спецификации), gate_api/api_client.py `gen_sign` — строка подписи.
- Официальный SDK WebSocket https://github.com/gate/gatews: go/constant.go (каналы), python/gate_ws/client.py (подпись
  «channel=…&event=…&time=…»), go/response_spot.go (модели уведомлений).
- Живая запись публичных ответов 04.11 (tests/data/cex_samples_2026_11_04/gate_*): сеть BSC — «BSC»; в
  /wallet/currency_chains есть поле withdraw_amount_min, которого нет в модели SDK; create_time_ms у spot.trades — строка
  с дробью («1793740740520.563000»); в spot.order_book_update поля full в записи нет; сервер не возвращает наш id в ответе
  на подписку без id (мы шлём id — номер пары в соединении).

## Уроки → как закрыты

| № | Урок | Как закрыт у Gate (файл:строка) |
| --- | --- | --- |
| 1 | Только документация, ссылка в каждой функции, нет — вопрос | Ссылка на раздел — в шапке каждого файла `src/venues/gate/*.rs` и в README; вопросы — docs/TASKS.md («Биржа Gate») |
| 2 | Справочник ≠ то, что биржа принимает | Перечень time_in_force не берётся из справочника пары (его там нет) — из документации ордера, order_rules.rs:68; итог проверяется первым ордером (TASKS, вопрос 4); поля живого ответа сверены с моделями (лишнее withdraw_amount_min — записано) |
| 3 | Тесты только на записанных ответах | tests/gate_real_data.rs: 9 тестов на живой записи 04.11, 4 приватных — #[ignore] с тем, что записать (tests/README.md) |
| 4 | Одна функция правил `order_rules::ready` у ордера и check_order; пробный ордер | order_rules.rs:105 (ready), trade.rs вызывает её, mod.rs check_order — её же; пробного ордера в спот API v4 нет → venue.order_test_first = да — ошибка настройки, order_rules.rs:106 |
| 5 | «Не исполнился — отменяем»: IOC | time_in_force только ioc / fok (остаток снимает биржа), order_rules.rs:68; если ответ не окончательный — опрос и отмена по сроку, order_state.rs:57 |
| 6 | Свой ID на каждую попытку, длина и алфавит по документации | text «t-» + метка попытки (мс) + ID сделки, ≤ 28 после «t-», [0-9A-Za-z_.-], order_rules.rs:139; вывод — withdraw_order_id ≤ 32, transfers.rs:48 |
| 7 | Итог — факт; сроки и лимиты истории — до отправки | Итог из полей ордера (filled_amount, filled_total, fee, fee_currency, gt_fee, point_fee), order_state.rs:141; venue.order_poll_ms, venue.order_final_wait_ms, venue.order_history_limit проверяются до отправки, order_rules.rs:114–115 |
| 8 | Сопоставление по адресу, имя сети BSC — настройкой | Адрес контракта только у сети venue.bsc_network (chains[].addr), networks_parse.rs:90–91; настройка — wallet.rs:195 |
| 9 | Большой справочник вне рабочих потоков, Arc, запрос одной монеты, не толкать пересборку | /spot/currencies (2,1 МБ) и withdraw_status — разбор в spawn_blocking (rest.rs:173, wallet.rs:160), справочник за Arc с кэшем venue.networks_fresh_ms; одна монета — coin_networks_of (wallet.rs:97, mod.rs:120); одна монета без копии — networks_parse.rs:65 |
| 10 | Статусы ввода/вывода, комиссия, минимум, точность, memo, счёт депозита | Флаги сети, delisted, withdraw_delayed — networks_parse.rs; комиссия withdraw_fix_on_chains, процент ≠ 0 — без комиссии (networks_parse.rs:95–98); точность — decimal currency_chains (networks_parse.rs:159) или пары; memo — отказ (deposit_addr.rs:88); депозит — на спот («DONE: Funds Credited to Spot Account»), deposits.rs:48, перевода нет (mod.rs, deposit_to_trading) |
| 11 | Комиссия по паре/ключу из API, монета — по факту | /wallet/fee?currency_pair= (fees.rs:108), монета комиссии — fee_currency ордера (GT при скидке GT), order_state.rs:141 |
| 12 | Стакан: снимки или изменения с номерами; лимиты подписок; пинг | spot.order_book — снимки N уровней (номер lastUpdateId, меньший — отбрасывается), ws_conn.rs:119; spot.ping раз в venue.ws_ping_ms; лимита подписок на соединение в документации нет — venue.ws_symbols_per_conn замером (вопрос); окна — U/u и снимок REST (tap_snap.rs) |
| 13 | Минимум выделений на кадр | Отсев по номеру, а не сравнением уровней (без копий уровней), ws_conn.rs:119; кадры стаканов в сырой журнал — только при raw.book_frames, ws_conn.rs:112 |
| 14 | Пересподписка тем же набором не перезапускает фон | subs.rs:121 (как MEXC, решение 131) |
| 15 | REST-лимиты, 429 — пауза, нет REST на горячем пути | reply.rs:53 (пауза по Retry-After или venue.rate_limit_pause_ms), rest.rs:136 (не слать до конца паузы); котировка — только из памяти (book.rs), комиссии — фоном |
| 16 | Баланс и ордера — приватный WS | spot.balances с подписью (account_update.rs:35, user_stream.rs); spot.orders не используется — итог ордера читается REST (ответ ioc окончательный) |
| 17 | Готовая фича: настройки, UI, «Проверить подключение», ARCHITECTURE | Каталог: src/settings/catalog/gate.rs (+ общие venue.* по площадке); SETTINGS.md, FIRST_RUN; «Площадки» — из реестра (main.rs), «Проверить подключение» — account.rs; ARCHITECTURE «Готовые фичи» |
| 18 | Секреты только в зашифрованном файле | gate.api_key, gate.api_secret — secrets::KNOWN; ключ в подписи — только в заголовке; в сырой журнал потока балансов ключ не попадает (user_stream.rs), отправленное не пишется |
| 19 | Права ключа: спот и ввод/вывод, без фьючерсов/маржи; белый список адресов | account.rs:62 (только key.mode = 1), account.rs:119 (запрет записи futures, delivery, margin, unified, options, loan, copy); белый список адресов вывода — включить в кабинете, ADDRESS_NOT_USED → InvalidAddress (errors.rs) |
| 20 | Ядро и шлюз не менять; кошелёк — строка маппинга | Ядро и шлюз не тронуты; регистрация — main.rs, секреты — secrets/mod.rs, каталог — одна строка в settings/catalog/mod.rs; строка «gate» на «Маппинге кошельков» появляется из реестра (api/wallets_view.rs, VenueKind::Cex) без новых настроек |

## Что не сделано / ждёт

- Приватные ответы не записаны (нужен ключ на живом боте) — tests/gate_real_data.rs #[ignore], список — tests/README.md.
- Замер нагрузки при «Стоп» (решение 109) — до включения в торговлю.
- Telegram: своё событие у Gate одно — exec.withdraw_fail_pending (вывод в статусе FAIL, см. ниже); остальные — общие
  события площадок (ошибки, статусы ввода/вывода, внешнее изменение баланса) с площадкой gate (фильтр notify.event.venues).

## Ответы Тиграна 04.11 (ветка router/cexfix, DECISIONS 135 «Дополнение»)

| № | Ответ | Как сделано |
| --- | --- | --- |
| 1 | Время жизни ioc ИЛИ fok — настройкой | Уже было: venue.order_time_in_force, проверка в order_rules.rs check_rules (одна функция у ордера и check_order); описание настройки, SETTINGS, FIRST_RUN — обновлены |
| 2 | DEP_CREDITED — зачислено, продавать сразу | deposits.rs: DONE и DEP_CREDITED → Credited |
| 3 | Вывод FAIL — не провал: ждать, сразу событие и журнал | transfers.rs: FAIL → Pending + withdraw_notice.rs (журнал venue.transfer Warn и Notice exec.withdraw_fail_pending, один раз на вывод; канал ядра — из subscribe_market) |
| 4 | Скидка GT — настройка: монета сделки / GT | venue.fee_deduction (coin / gt), fees.rs Rates::parse/plan (taker_fee или gt_taker_fee; сверка с gt_discount — не совпадает → котировки нет), book.rs: coin + покупка — комиссия в монете (ядро уменьшает количество к выводу, core/costs.rs), иначе — USDT; факт — order_state.rs как было. Нехватка GT — в документации API нет (TASKS, вопрос 1) |
