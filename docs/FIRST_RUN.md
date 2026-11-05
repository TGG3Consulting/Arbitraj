# FIRST_RUN — Таблица первого запуска (СОВЕТУЕМЫЕ стартовые значения)

> **Это советы, а не готовые настройки.** Значений по умолчанию у движка нет и не появляется:
> не задана настройка — функция, которой она нужна, не работает (CLAUDE.md, docs/SETTINGS.md).
> Эти значения Тигран подтверждает и сохраняет сам на экране «Настройки» (с журналом изменений).
> Автоматической записи настроек по этой таблице нет.
>
> Источник данных — `config/first_run.toml` (его отдаёт `GET /api/first-run`, показывает экран «Первый запуск»).
> Правило «только факты» (решение 23): лимиты площадок/протоколов и адреса — задокументированные, со ссылкой на источник;
> параметры поиска — ориентиры из `docs/SETTINGS.md`; капитал-зависимые `money.*` / `risk.*` — метод, без готового числа.
>
> Готовые значения с областью можно предзаполнить в форму настроек кнопкой «Заполнить форму настроек» на экране
> «Первый запуск»; строки-методы (деньги и риск) не предзаполняются — их вводит Тигран.

## Связь: узел BSC и справочник пулов

С этого начинается первый запуск: бесплатный публичный узел и бесплатный индексатор — чтобы DEX заработал без регистраций.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| bsc.node_ws_url | вся система | `wss://bsc-rpc.publicnode.com` | Бесплатный публичный узел без ключа, принимает большие фильтры logs. Резервный и платный узлы — отдельно | Офсписок BNB Chain; решение 79 |
| bsc.pool_events_source | вся система | `ws` | На публичном узле подписка logs легче, чем квитанции каждого блока (receipts) | Решение 79 |
| index.api_url | по площадке DEX | `https://api.geckoterminal.com/api/v2` | GeckoTerminal без ключа — справочник пулов работает сразу. Ключ CoinGecko подставляется позже | Документация CoinGecko/GeckoTerminal |
| index.network | по площадке DEX | `bsc` | id сети BSC у индексатора | Индексатор, GET /networks |
| index.requests_per_min | по площадке DEX | `10` | Лимит бесплатного GeckoTerminal; с ключом Demo — до 100 | Документация CoinGecko |
| index.page_size | по площадке DEX | `20` | Максимум пулов на странице | Документация: «up to 20 pools per page» |
| index.pages_per_token | по площадке DEX | `2` | На первой странице у части монет не все пулы с USDT | Запись 2026-10-24; >10 страниц — тариф Analyst |
| index.key_header | по площадке DEX | *(не задавать)* | Первый запуск — без ключа. С ключом: Demo `x-cg-demo-api-key`, Pro `x-cg-pro-api-key` | Документация CoinGecko |

## Расчёт кандидатов — параллельность и срок (решение 88)

Расчёты кандидатов идут параллельно (свой на каждый); узел ими почти не нагружается (счёт в памяти, цена газа — общий кэш на блок). Срок не даёт расчёту зависнуть на медленном или перегруженном узле.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| profit.calc_timeout_ms | вся система | `2000` | Общий срок одного расчёта: вышел — отказ «не уложился в срок», направление встанет снова следующим тиком. С запасом на обычный расчёт, но ловит зависание, когда узел не отвечает | Решение 88 |
| profit.calc_concurrency | вся система | `0` | 0 — без предела (свой расчёт на каждый кандидат, ни один не ждёт). Если расчёты заметно грузят процессор сервера — поставить по числу ядер | Решение 88 |
| dex.quote_cpu_max | bsc_dex | `0` | 0 — без предела. Если при включённой MEXC процессор в потолке — число ядер сервера минус 2–4: котировки DEX ждут места, а приём данных и сроки расчёта не страдают | Решение 131 |

## Журнал отказов кандидатов (решение 155)

Отказ кандидата с той же причиной по направлению — одна строка журнала со счётчиком повторов («×N, с … по …»), а не новая строка на каждый расчёт.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| journal.repeat_flush_ms | вся система | `5000` | Строка повторов в базе обновляется не чаще раза в 5 с: при ~11 отказах в секунду — одна запись в базу на направление раз в 5 с вместо ~55; итог не теряется (отложенное обновление и закрытие при смене причины). Не задана — каждый отказ отдельной строкой, как раньше | Решение 155 |

## Нативная монета BNB

У BNB нет адреса контракта — он сопоставляется с обёрткой WBNB.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| dex.native_wrapped | по площадке DEX | `0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c` | WBNB — обёртка BNB; без неё нативная монета не торгуется | weth9 в развёртывании Universal Router PancakeSwap/Uniswap для BSC |
| dex.native_ticker | по площадке DEX | `BNB` | Вместе с dex.native_wrapped говорит, какая монета биржи нативная | Тикер нативной монеты BSC |

## Направление DEX ↔ DEX (замкнутые циклы) — фаза сигнала

Второе направление (решение 85). На старте — только сигнал, без авто-исполнения. Включатели оставляем выключенными: их включает Тигран.

| Настройка | Область | Советуем / метод | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| dex_dex.enabled | по площадке DEX | *метод:* оставить выключенным; включает Тигран, когда готов смотреть сигнал | Фаза сигнала без авто-исполнения — под контролем человека | — |
| dex_dex.base_tokens | по площадке DEX | `0x55d398326f99059fF775485246999027B3197955, 0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c` | USDT и WBNB: у обеих курс BNB → базовая берётся из графа | USDT и WBNB в сети BSC |
| dex_dex.max_cycle_len | по площадке DEX | `3` | Баланс покрытия и скорости перебора | Ориентир 2–4 (docs/SETTINGS.md) |
| dex_dex.max_cycles | по площадке DEX | `20` | Предел циклов на базовую монету — держит скорость | Ориентир (docs/SETTINGS.md) |
| dex_dex.probe_input_base | по площадке DEX | *метод:* небольшой объём под капитал; ориентир 100 (USDT) / 0,2 (WBNB); подбирает Тигран | Верхняя граница входа сигнала; сигнал — на оптимальном входе в её пределах (решение 124) | — |
| dex_dex.min_profit_base | по площадке DEX | *метод:* не задавать (порог 0) | На фазе сигнала смотрим все циклы | — |
| dex_dex.min_profit_usd | по площадке DEX | *метод:* задать перед dex_dex.execute (обязательна для исполнения), значение — Тигран | Порог исполнения цикла в USDT для любой базовой (решение 124) | — |
| dex_dex.execute | по площадке DEX | *метод:* оставить выключенным; включает Тигран после сухих сделок цикла | Авто-исполнение отдельно от сигнала — под контролем человека | — |
| dex_dex.size_step_base | по площадке DEX | *метод:* шаг в единицах базовой; ориентир 0.01 (USDT) / 0.0001 (WBNB) | Шаг подбора входа цикла (решение 124) | — |
| dex_dex.scan_min_interval_ms | по площадке DEX | *метод:* не задавать (проход на каждом новом состоянии пулов); задать, если поиск грузит процессор | Как signal.eval_min_interval_ms | — |

## Окна (решение 125)

Сырые кадры стакана и каждая сделка биржи по монете, пока её направление проходит расчёт прибыльности и до конца нашей сделки. Ориентиры, не потолки площадок.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| window.enabled | вся система | *метод:* включить («да»), когда заданы остальные window.* | Окна — запись наблюдения, на торговлю не влияют; сырые данные копятся с первого сигнала | Решение 125 |
| window.min_hold_ms | вся система | `2000` | Направление мигает между «кандидатом» и отказом от тика к тику; 2 с держат одно окно через короткие провалы, не растягивая его надолго. | Ориентир (решение 125) |
| window.queue_max | вся система | `10000` | Запас на всплеск кадров (стакан + лента) при медленном диске: кадр стакана BingX depth20 — ~2 КБ текста, 10000 кадров — ~20 МБ памяти на окно в худшем случае; потери видны в списке. | Ориентир (решение 125) |
| window.max_mb | вся система | `2048` | Окно с минутой записи — сотни КБ, с долгой сделкой — единицы МБ (оценка в решении 125); 2 ГБ — тысячи окон, сервер Rise-3 — 2×1.92 ТБ. | Ориентир (решение 125) |
| window.keep_days | вся система | `14` | Две недели окон на разбор и подбор порогов; старше — уборка. | Ориентир (решение 125) |
| window.zstd_level | вся система | `3` | Быстрое сжатие в потоке без нагрузки на процессор; поднять, если места мало. | Документация zstd: уровень по умолчанию библиотеки — 3 |
| window.cleanup_s | вся система | `3600` | Уборка раз в час; при нехватке места уборка идёт сразу. | Ориентир (как episode.cleanup_s) |
| venue.trades_interval | площадка MEXC | `10ms` | Ближе к сырому потоку: кадр за 10 мс, в нём каждая сделка отдельно с tradeId. | Официальная документация MEXC, Trade Streams: (100ms\|10ms) |
| venue.window_depth_interval | площадка MEXC | `10ms` | Ближе к сырому потоку изменений стакана (ответ Тиграна по решению 125) | Официальная документация MEXC, Diff.Depth Stream: (100ms\|10ms) |
| venue.window_depth_limit | площадка MEXC | `1000` | Глубина снимка, которую берёт сама процедура MEXC «How to Properly Maintain a Local Copy of the Order Book»; предел — 5000 | Документация MEXC: Order Book «max 5000», процедура — limit=1000 |

## Упущено из-за потолка (решение 141)

Фоновый расчёт «без потолка»: сколько дала бы сделка без лимитов money.*, свободного остатка и виртуального бюджета — только ликвидность и предел card.max_search_usdt. На торговлю не влияет; стоимость — процессор (docs/notes/uncapped-2026-11-04.md).

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| uncapped.enabled | вся система | *метод:* включить («да»), когда заданы остальные uncapped.* и card.max_search_usdt | Наблюдение, на торговлю не влияет; включается отдельно, чтобы видеть цену по процессору | Решение 141 |
| uncapped.max_concurrent | вся система | `1` | Не больше одного ядра и одного места dex.quote_cpu_max | Ориентир (решения 129–131, 141) |
| uncapped.min_interval_ms | вся система | `30000` | Раз в 30 с по направлению хватает, чтобы поймать пик эпизода; сделки — без интервала | Ориентир (решение 141) |
| uncapped.queue_max | вся система | `64` | Повторы направления сливаются; запас на всплеск, пропуски видны счётчиком | Ориентир (решение 141) |
| uncapped.max_evals | вся система | `160` | На записи CAKE оптимум тот же, что при 300 (полный поиск — 213–238 расчётов), на 80 отходит | Замер, tests/core_uncapped.rs |
| uncapped.ladder_usdt | вся система | `100, 200, 500, 1000, 2000, 5000, 10000` | От нынешнего потолка до размеров, где кончается ликвидность пулов | Задача Тиграна 04.11.2026 |
| uncapped.episode_gap_ms | вся система | `120000` | Четыре интервала: короткий провал кандидата не дробит эпизод | Ориентир (решение 141) |
| uncapped.keep_days | вся система | `30` | Месяц для анализа; строка — несколько КБ | Ориентир (решение 141) |

## Лимиты биржи BingX

Потолки площадки из официальной документации — безопасны для старта.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| venue.recv_window_ms | площадка BingX | `5000` | Максимальное окно подписанного запроса | Документация BingX: не больше 5000 |
| venue.ws_subs_per_sec | площадка BingX | `10` | Предел темпа подписок WebSocket | Документация BingX: не больше 10 |
| venue.fee_requests_per_sec | площадка BingX | `2` | Предел запросов ставок комиссии | Документация BingX: не больше 2 |

## Биржа Gate (решение 135)

Советы, не значения по умолчанию. Включать — после замера нагрузки при «Стоп» (решение 109).

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| venue.rest_url | площадка Gate | `https://api.gateio.ws/api/v4` | Адрес с префиксом /api/v4 — путь входит в подпись | Gate API v4, Access URL |
| venue.ws_url | площадка Gate | `wss://api.gateio.ws/ws/v4/` | Стаканы, окна и поток балансов | Spot WebSocket v4; запись 04.11 |
| venue.bsc_network | площадка Gate | `BSC` | Имя сети BNB Smart Chain у Gate | Живая запись 04.11 (/spot/currencies, currency_chains CAKE) |
| venue.depth_levels | площадка Gate | `20` | Как у других бирж | spot.order_book: 5, 10, 20, 50, 100 |
| venue.book_interval | площадка Gate | `100ms` | Самый частый снимок | spot.order_book: 100ms, 1000ms |
| venue.order_time_in_force | площадка Gate | `ioc` | «Не исполнился — отменяем»: остаток снимает биржа; `fok` тоже допустим — выбор настройкой (Тигран, 04.11) | POST /spot/orders, time_in_force |
| venue.fee_deduction | площадка Gate | *метод:* `coin` — скидка GT в кабинете выключена; `gt` — включена и есть GT. Должно совпадать с кабинетом, иначе котировок нет | Расчёт — по выбранной ставке и монете комиссии; факт — из ордера | GET /wallet/fee: gt_discount, gt_taker_fee; решение Тиграна 04.11 |
| venue.deposit_info_requests_per_sec | площадка Gate | `2` | Минимум депозита по монетам подписки — фоном, с большим запасом от лимита Gate | Wallet «The Others: 200r/10s per endpoint»; решение 143 |
| venue.deposit_info_refresh_s | площадка Gate | `3600` | Раз в час перепроверить минимумы депозита | решение 143 |
| venue.deposit_min_absent_ok | площадка Gate | `да` | Пока минимум депозита монеты не получен фоном — 0 (решение 143) | min_deposit_amount только в адресе депозита |
| venue.order_test_first | площадка Gate | `нет` | Пробного ордера у Gate нет («да» — ошибка настройки) | Спот API v4 |
| venue.ws_ping_ms | площадка Gate | `10000` | spot.ping сбрасывает таймер простоя | Официальный пример: раз в 10 с |
| venue.fee_requests_per_sec | площадка Gate | `5` | Четверть предела /wallet/fee | «Frequency limit rule»: 200 за 10 с |
| venue.rate_limit_pause_ms | площадка Gate | `10000` | Целое окно лимита после 429 (Retry-After нет) | «Frequency limit rule»: окно 10 с |
| venue.order_history_limit | площадка Gate | `100` | Поиск своего ордера по text | GET /spot/orders: «default 100» |
| venue.window_diff_interval | площадка Gate | `100ms` | Глубина 100 | spot.order_book_update: 20ms / 100ms |
| venue.window_depth_limit | площадка Gate | `100` | Снимок той же глубины, что изменения | spot.order_book_update 100ms — глубина 100 |
| venue.ws_symbols_per_conn | площадка Gate | *метод:* начать с 20, замерить при «Стоп», при обрывах уменьшить | Предела в документации нет | Spot WebSocket v4 (вопрос Тиграну) |
## Биржа Bybit (решение 136)

Адреса и потолки — из официальной документации Bybit V5; остальное — ориентиры. Депозит (решение Тиграна 04.11): в кабинете
Bybit назначить зачисление на UNIFIED (Set Deposit Account); бот перевод FUND → UNIFIED не делает.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| venue.rest_url | площадка Bybit | `https://api.bybit.com` | Основной адрес mainnet | Integration Guidance: «both endpoints are available» |
| venue.rest_fallback_url | площадка Bybit | `https://api.bytick.com` | Второй адрес mainnet — только при сбое сети | Integration Guidance |
| venue.ws_url | площадка Bybit | `wss://stream.bybit.com/v5/public/spot` | Публичный поток спота | Connect |
| venue.ws_private_url | площадка Bybit | `wss://stream.bybit.com/v5/private` | Приватный поток | Connect |
| venue.quote_asset | площадка Bybit | `USDT` | Котируемая монета пар | instruments-info: quoteCoin |
| venue.order_time_in_force | площадка Bybit | `IOC` | Остаток снимает биржа (правило «не исполнился — отменяем») | enum timeInForce |
| venue.depth_levels | площадка Bybit | `50` | 50 уровней, кадры раз в 20 мс | Orderbook → Depths → Spot (1, 50, 200, 1000) |
| venue.ws_symbols_per_conn | площадка Bybit | `100` | Предел args на соединение спота | Connect: «up to 100 args for a single connection» |
| venue.ws_ping_ms | площадка Bybit | `20000` | Совет Bybit — ping раз в 20 с | Connect: «every 20 seconds» |
| venue.my_trades_limit | площадка Bybit | `100` | Наибольший limit исполнений ордера | Get Trade History: «[1, 100]» |
| venue.fee_requests_per_sec | площадка Bybit | `5` | Потолок fee-rate | Rate Limit: /v5/account/fee-rate 5/s |
| venue.bsc_network | площадка Bybit | *метод:* значение поля chain сети BSC из живого coin/query-info | В документации имени сети BSC нет | coin/query-info |
| venue.withdraw_account | площадка Bybit | *метод:* `UTA` (торговый баланс — на UNIFIED, депозит туда же); решает Тигран | Торговый баланс — на UNIFIED | Withdraw: accountType |
| venue.deposit_to_spot | площадка Bybit | *метод:* не задавать — депозит назначен на UNIFIED в кабинете; «да» — только запасной вариант при депозите на FUND (право Wallet: AccountTransfer) | Счёт зачисления API прочитать не даёт — проверяет Тигран в кабинете | Set Deposit Account; решение Тиграна 04.11 |
| venue.book_seq_strict | площадка Bybit | *метод:* да — на живом потоке `u` изменений шёл строго +1 (запись 04.11); частые пересинхронизации в журнале — нет | Потерянный кадр виден сразу | Orderbook (+1 не обещано); решение Тиграна 04.11 |
| venue.recv_window_ms | площадка Bybit | `5000` | Значение по умолчанию Bybit | Integration Guidance: «default value is 5,000» |
## Биржа KuCoin (решение 137)

Из официальной документации KuCoin (Classic) и живых ответов 04.11.2026. Перед торговлей — замер нагрузки при «Стопе» (решение 109).
Решение Тиграна (04.11): счёт Classic, спот HF; адрес депозита BSC создаётся в кабинете с зачислением на TRADE —
`venue.deposit_to_spot` не задавать (бот берёт адрес TRADE; адрес только MAIN — отказ с причиной); `venue.withdraw_from_trade`
= да (вывод только с main — перед ним перевод trade → main). По желанию: `venue.order_test_first` = да (пробный ордер до
проверки шагов на живом), `venue.balance_events` = да (поток аккаунта).

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| venue.rest_url | площадка KuCoin | `https://api.kucoin.com` | Основной адрес REST Classic | Документация KuCoin (Get Public Token), примеры SDK |
| venue.deposit_min_absent_ok | площадка KuCoin | `да` | depositMinSize = null у большинства монет — считать 0 (решение 143) | /api/v3/currencies, записанный ответ 04.11 |
| venue.bsc_network | площадка KuCoin | `bsc` | chainId сети BSC (принимают вывод и адрес депозита) | Живой ответ /api/v3/currencies/CAKE 04.11 |
| venue.api_key_version | площадка KuCoin | `3` | Сверить со страницей ключа; «Проверить подключение» сравнит с apiVersion | Документация: Authentication |
| venue.order_time_in_force | площадка KuCoin | `IOC` | Неисполненное снимает биржа | Документация: Add Order, timeInForce |
| venue.depth_levels | площадка KuCoin | `50` | Глубина для котировки; 5 — меньше нагрузки | Документация: level2Depth5 / level2Depth50 |
| venue.ws_subs_per_sec | площадка KuCoin | `10` | Потолок темпа подписок | Документация: 100 сообщений за 10 с |
| venue.window_depth_limit | площадка KuCoin | `100` | Снимок для изменений стакана в окнах | Документация: Get Part OrderBook, size 20 или 100 |
| venue.withdraw_from_trade | площадка KuCoin | `да` | Вывод только с main: перед ним trade → main (право FlexTransfers) | Код 115007; решение Тиграна 04.11 |
| venue.deposit_to_spot | площадка KuCoin | *метод:* не задавать — адрес депозита зачисляет на TRADE | Перевод main → trade не нужен | Get Deposit Address V3 (to); решение Тиграна 04.11 |

## Биржа Bitget (решение 162)

Из официальной документации Bitget Classic v2; живых ответов ещё нет (из среды разработки api.bitget.com закрыт). Счёт —
Classic (не UTA: «Проверить подключение» откажет). Адреса вывода — заранее в адресную книгу кабинета Bitget (код 11000),
ключ с выводом — только с привязкой IP сервера. Перед торговлей — замер нагрузки при «Стопе» (решение 109).

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| venue.rest_url | площадка Bitget | `https://api.bitget.com` | Основной адрес REST Classic v2. | Classic → REST API: «https://api.bitget.com»; SDK consts.py API_URL |
| venue.ws_url | площадка Bitget | `wss://ws.bitget.com/v2/ws/public` | Стаканы books5/books15. | Classic → WebSocket intro: публичный «wss://ws.bitget.com/v2/ws/public» |
| venue.quote_asset | площадка Bitget | `USDT` | Котируемая монета пар. | Get Symbol Info: quoteCoin; minTradeUSDT — минимум суммы в USDT |
| venue.depth_levels | площадка Bitget | `15` | Глубина для котировки на объём; 5 — меньше нагрузки. | Depth Channel: books1 (10 мс), books5 и books15 (200 мс) — снимками |
| venue.ws_symbols_per_conn | площадка Bitget | `49` | Рекомендация документации — меньше 50 каналов на соединение. | WebSocket intro: «less than 50 channels in one connection», «Max 1000 channel subscription/connection» |
| venue.ws_subs_per_sec | площадка Bitget | `10` | Потолок темпа сообщений; подписка идёт пачками ≤ 4096 байт. | WebSocket intro: до 10 сообщений клиента в секунду (ping, login, subscribe) |
| venue.ws_ping_ms | площадка Bitget | `30000` | Совет документации — раз в 30 с. | WebSocket intro: строка «ping» каждые 30 с, без ping 2 минуты — разрыв |
| venue.ws_silence_ms | площадка Bitget | *метод:* больше venue.ws_ping_ms с запасом на ответ «pong» (например, два интервала ping) — решает Тигран | Без кадров дольше — соединение оборвано. | WebSocket intro: ответ «pong» на каждый «ping» |
| venue.order_time_in_force | площадка Bitget | `ioc` | Правило Тиграна: неисполненный остаток снимает биржа. | Place Order: force gtc / post_only / fok / ioc |
| venue.order_test_first | площадка Bitget | *метод:* не задавать (или «нет»): пробного ордера у Bitget Classic нет — «да» даёт отказ ордера | Правила ордера проверяет order_rules::ready. | Spot → Trade: среди эндпоинтов нет test order |
| venue.bsc_network | площадка Bitget | *метод:* значение поля chain сети BSC из живого ответа GET /api/v2/spot/public/coins?coin=CAKE | В документации имени сети BSC нет. | Get Coin Info: chain «Chain name» (в примере только BTC) |
| venue.deposit_min_absent_ok | площадка Bitget | *метод:* решает Тигран после живого ответа: у монет с minDepositAmount «0» или пусто — считать минимум 0 (да) или не торговать с переводом на Bitget | Урок 21: «0» и пусто у биржи могут значить разное. | Get Coin Info: minDepositAmount; смысл «0» не описан |
| venue.history_window_s | площадка Bitget | *метод:* с запасом больше самого долгого ожидания вывода и зачисления (например, сутки — 86400); решает Тигран | Окно поиска статусов депозита и вывода. | Get Deposit/Withdrawal Records: startTime и endTime обязательны, предел промежутка не указан |
| venue.rate_limit_pause_ms | площадка Bitget | *метод:* пауза после 429 — Retry-After в документации нет, окно лимитов — секунда (например, 1000–2000); решает Тигран | Не слать дальше после «слишком часто» (урок 15). | Classic → REST API: «429»; лимиты «N times/1s» |
| venue.deposit_to_spot | площадка Bitget | *метод:* не задавать, если в кабинете (или Modify Deposit Account) зачисление монеты назначено на spot; «да» — если депозит приходит на фонд (право ключа «Transfer»); проверить первым депозитом | На какой счёт зачисляется депозит, в документации Classic не сказано. | Modify Deposit Account (accountType spot / funding …); Transfer p2p → spot |
| venue.withdraw_from_trade | площадка Bitget | *метод:* «да» — если малый вывод со спота отклонён (вывод с фонда); иначе не задавать | Проверить малым выводом. | Withdraw: с какого счёта списывается — не описано |
## Биржа OKX (решение 161)

Из официальной документации OKX API v5 (живых ответов ещё нет — из среды разработки OKX закрыт; сверить на сервере бота, tests/README.md, раздел OKX). Перед торговлей — замер нагрузки при «Стопе» (решение 109). Режим счёта Spot (acctLv 1), ключ основного счёта с правами Read, Trade, Withdraw, привязан к IP; адреса вывода — заранее в кабинете OKX.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| venue.rest_url | площадка OKX | *метод:* Домен — по сущности счёта, не выбирается ботом: основной https://openapi.okx.com, EEA — https://eea.okx.com, US — https://us.okx.com. Какой у счёта — видно по сайту входа (my.okx.com — EEA, app.okx.com — US/AU). | Запросы на чужой домен сущности не проходят. | Документация OKX v5, Overview → Production Trading Services: «Using openapi.okx.com will not work for the regions below» |
| venue.ws_url | площадка OKX | *метод:* Публичный WebSocket той же сущности: основной wss://ws.okx.com/ws/v5/public (у EEA / US — свой домен из того же раздела документации). | Стаканы books5 / books. | Документация OKX v5, Overview → Production Trading Services; порт 8443 закрыт с 31.10.2026 (журнал изменений) |
| venue.ws_business_url | площадка OKX | *метод:* WebSocket business той же сущности: основной wss://ws.okx.com/ws/v5/business. | Зачисление депозитов событием с хешем (venue.balance_events). | Документация OKX v5, Deposit info channel: «URL Path /ws/v5/business (required login)» |
| venue.balance_events | площадка OKX | `да` | Депозит закрывается событием, а не опросом статуса. | Документация OKX v5, Deposit info channel |
| venue.bsc_network | площадка OKX | *метод:* Часть chain после «<монета>-» сети BSC в живом ответе GET /api/v5/asset/currencies?ccy=CAKE (ожидается BSC — в документации только пример USDT-ERC20). | Название сети одно на все монеты у ядра; chain OKX бот берёт из справочника. | Документация OKX v5, Get currencies: chain «e.g. USDT-ERC20» |
| venue.deposit_min_absent_ok | площадка OKX | *метод:* По живому ответу currencies: minDep у монет BSC приходит — не задавать; пустой у многих — решение Тиграна (что значит пустой minDep, документация не говорит). | Решение 143: «0» и пустое у биржи могут значить разное. | Документация OKX v5, Get currencies: minDep |
| venue.depth_levels | площадка OKX | `400` | Глубина для котировки на объём; при нагрузке процессора — 5 (снимки books5). | Документация OKX v5, Order book channel: books — 400 уровней + изменения каждые 100 мс; books5 — 5 уровней |
| venue.ws_ping_ms | площадка OKX | `20000` | Сервер рвёт молчащее соединение через 30 с. | Документация OKX v5, Overview → WebSocket → Connect: «Set a timer of N seconds … N is less than 30» |
| venue.ws_silence_ms | площадка OKX | `40000` | Пинг через 20 с тишины + 20 с на pong. | Документация OKX v5, Connect: «If the response message is not received within N seconds, please raise an error or reconnect» |
| venue.ws_connects_per_sec | площадка OKX | `3` | Потолок темпа подключений. | Документация OKX v5, Connect: «Connection limit: 3 requests per second (based on IP)» |
| venue.order_type | площадка OKX | `ioc` | Правило Тиграна: неисполненный остаток снимает биржа. | Документация OKX v5, Place order: ordType ioc / fok |
| venue.fee_requests_per_sec | площадка OKX | `2` | Потолок лимита эндпоинта (целое вниз). | Документация OKX v5, Get fee rates: «5 requests per 2 seconds» |
| venue.rate_limit_pause_ms | площадка OKX | `2000` | Пауза на окно лимита после 50011 / HTTP 429. | Документация OKX v5, Rate Limits: лимиты почти всех эндпоинтов — за 2 с; Retry-After не описан |
| venue.withdraw_from_trade | площадка OKX | `да` | Торговля — на Trading, вывод — с Funding: перед выводом перевод 18 → 6. | Документация OKX v5, Withdrawal: «Only supported withdrawal of assets from funding account» |
| venue.deposit_to_spot | площадка OKX | *метод:* Не задавать, если в кабинете у адреса BSC зачисление на Trading account (to = 18). «да» — только если адрес зачисляет на Funding (6): тогда после зачисления перевод 6 → 18. | Адрес Funding при выключенном переводе бот не отдаёт — отказ с причиной. | Документация OKX v5, Get deposit address: to «6: Funding account 18: Trading account» |
| venue.transfer_poll_ms | площадка OKX | `1000` | Опрос итога перевода без упора в лимит. | Документация OKX v5, Funds transfer: «2 requests per second»; Get funds transfer state |
| venue.transfer_wait_ms | площадка OKX | `30000` | Не дождались — итог неизвестен, ручная проверка. | Документация OKX v5, Funds transfer: «does not necessarily reflect the actual transfer result» |
| venue.order_test_first | площадка OKX | *метод:* Не задавать: пробного ордера для спота у OKX нет (order-precheck — только Multi-currency и Portfolio margin); «да» — ошибка настройки. | check_order — только правила пары. | Документация OKX v5, Order precheck |
## Биржа Binance (решение 160)

Из официальной документации Binance (спот API, WebSocket Streams, WebSocket API, Wallet SAPI); живых ответов ещё нет —
сверить на первой записи (tests/README.md, раздел «Binance»). Перед торговлей — замер нагрузки при «Стопе» (решение 109).
Ключ — HMAC; вывод Binance разрешает только ключу с ограничением по IP; адрес кошелька DEX (BSC) — заранее в белый список.
По желанию: `venue.balance_events` = да (поток аккаунта через WebSocket API).

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| venue.rest_url | площадка Binance | `https://api.binance.com` | Спот и Wallet SAPI | rest-api «General API Information» |
| venue.rest_fallback_url | площадка Binance | `https://api-gcp.binance.com` | Резерв только при сбое сети | rest-api «General API Information» |
| venue.ws_url | площадка Binance | `wss://stream.binance.com:9443` | Без пути: бот добавляет /stream и /ws/<поток> | web-socket-streams |
| venue.ws_private_url | площадка Binance | `wss://ws-api.binance.com:443/ws-api/v3` | Поток аккаунта (listenKey устарел) | websocket-api, CHANGELOG 2025-04-07 |
| venue.bsc_network | площадка Binance | `BSC` | Код сети вывода и депозита — сверить на живом getall | Wallet «All Coins' Information» (пример) |
| venue.recv_window_ms | площадка Binance | `5000` | Совет документации | rest-api «Request Security» |
| venue.order_time_in_force | площадка Binance | `IOC` | Остаток снимает биржа | enums.md «Time in force» |
| venue.order_test_first | площадка Binance | `да` | Ловит PERCENT_PRICE_BY_SIDE и PRICE_RANGE | rest-api «Test new order» |
| venue.depth_levels | площадка Binance | `20` | Наибольшая глубина частичного стакана | Partial Book Depth: 5, 10, 20 |
| venue.book_interval | площадка Binance | `100ms` | Самый частый снимок (одинаковые ядру не отдаются) | Partial Book Depth: 1000ms или 100ms |
| venue.ws_subs_per_sec | площадка Binance | `2` | Лимит 5 сообщений/с включая pong | WebSocket Limits |
| venue.ws_streams_per_msg | площадка Binance | *метод:* начать с 50; отказ SUBSCRIBE — уменьшить | Предела в документации нет | SUBSCRIBE |
| venue.ws_symbols_per_conn | площадка Binance | `200` | Мало соединений (300 подключений за 5 мин с IP) | WebSocket Limits: 1024 потока |
| venue.my_trades_limit | площадка Binance | `1000` | Все сделки ордера одним запросом | myTrades: max 1000 |
| venue.fee_requests_per_sec | площадка Binance | `1` | Вес 20 — 1200 в минуту; сверить с rateLimits | Query Commission Rates |
| venue.fee_deduction | площадка Binance | *метод:* `coin` — скидки BNB нет; `bnb` — включена и BNB держится | Расчёт по выбранной ставке, факт — из fills | faqs/commission_faq; решение 158 |
| venue.rate_limit_pause_ms | площадка Binance | `60000` | Если Retry-After не пришёл (SAPI) | rest-api «LIMITS» |
| venue.withdraw_wallet | площадка Binance | `spot` | Торговля на споте | Withdraw: walletType |
| venue.withdraw_history_min_ms | площадка Binance | `6000` | Вес 18000 из 180000 UID в минуту | Withdraw History; General Info |
| venue.window_depth_limit | площадка Binance | `1000` | Снимок для изменений стакана окна | Order book: limit ≤ 5000 |
| venue.deposit_dust_as_min | площадка Binance | *метод:* решение Тиграна; до него не задавать | Смысл depositDust не описан | All Coins' Information |
| venue.deposit_to_spot | площадка Binance | *метод:* да — если депозит в кабинете на Фонд; проще выставить Спот | Торговля только на споте | Withdraw walletType; Universal Transfer |

## Биржа Phemex (решение 168)

Значения из официальной документации Phemex (phemex-docs.github.io). Живых ответов ещё нет — имя сети BSC, статус пары, счёт зачисления депозита задаёт Тигран по живым ответам. Ключ «Read & Trade & Withdraw» — с привязкой IP сервера; адреса вывода (кошелёк бота, адреса других бирж) — заранее в «withdrawal address list» кабинета Phemex. Перед торговлей — замер нагрузки при «Стопе» (решение 109).

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| venue.rest_url | площадка Phemex | `https://api.phemex.com` | Основной адрес REST (vapi.phemex.com — только для IP из белого списка Phemex). | General API information: public user endpoints — Rest API «https://api.phemex.com» |
| venue.ws_url | площадка Phemex | `wss://ws.phemex.com` | Стаканы и поток аккаунта — один адрес. | General API information: Websocket «wss://ws.phemex.com (further URI or querystring not allowed)» |
| venue.quote_asset | площадка Phemex | `USDT` | Котируемая монета пар. | Query product information: спот-пары sXXXUSDT; fee-rate по quoteCurrency |
| venue.pair_status_listed | площадка Phemex | *метод:* значение поля status пары CAKE из живого ответа GET /public/products (в примерах CCXT — «Listed»; в официальной документации поле не описано) | Без него справочник пар не загружается (торгуемость не угадываем). | Query product information: перечня полей нет |
| venue.depth_levels | площадка Phemex | `30` | Вся глубина канала для котировки на объём. | Subscribe orderBook: «depth = 30 and interval = 20ms» |
| venue.ws_symbols_per_conn | площадка Phemex | `20` | Предел документации. | API Rate Limits: «Each connection has subscription limit to 20 in maximum» |
| venue.ws_max_conns | площадка Phemex | `5` | Потолок документации; при venue.balance_events = да одно соединение — поток аккаунта. | API Rate Limits: «Each Client has concurrent connection limit to 5 in maximum» |
| venue.ws_subs_per_sec | площадка Phemex | `10` | Половина предела — запас на server.ping. | API Rate Limits: «Each connection has throttle limit to 20 request/s» |
| venue.ws_ping_ms | площадка Phemex | `5000` | Совет документации. | Heartbeat: «interval less than 30 seconds», «recommended to be set as 5 seconds» |
| venue.ws_silence_ms | площадка Phemex | `15000` | Три интервала ping. | Heartbeat: «reconnect … if don't receive messages in 3 heartbeat intervals» |
| venue.recv_window_ms | площадка Phemex | `60000` | Совет документации; тот же срок у входа WebSocket. | REST request header: expiry «normally it should be (Now() + 1 minute)»; user.auth — не больше 2 минут |
| venue.order_time_in_force | площадка Phemex | `ImmediateOrCancel` | Правило Тиграна: неисполненный остаток снимает биржа. | Place order: timeInForce GoodTillCancel, PostOnly, ImmediateOrCancel, FillOrKill |
| venue.order_test_first | площадка Phemex | *метод:* не задавать (или «нет»): пробного ордера у Phemex нет — «да» даёт отказ ордера | Правила ордера проверяет order_rules::ready. | Spot REST API: среди эндпоинтов нет test order |
| venue.bsc_network | площадка Phemex | *метод:* значение chainName сети BSC из живого ответа chainCfg?currency=CAKE (в документации примеры только BTC, ETH, TRX, EOS) | В документации имени сети BSC нет. | Query deposit chain settings: chainName |
| venue.deposit_min_absent_ok | площадка Phemex | *метод:* решает Тигран после живого ответа: у монет с minAmountRv «0» или пусто — считать минимум 0 (да) или не торговать с переводом на Phemex | Урок 21: «0» и пусто у биржи могут значить разное. | Query deposit chain settings: minAmountRv необязателен, в примере «0» |
| venue.networks_requests_per_min | площадка Phemex | *метод:* с запасом от предела группы Others 100/мин (её делят балансы, вывод, история, ставки) — например 30; решает Тигран | Круг ~300 монет × 2 запроса при 30/мин — около 20 минут. | Rate limits: Others «100/minute»; сети — по одной монете (chainCfg + asset/info) |
| venue.history_rows | площадка Phemex | *метод:* больше числа депозитов и выводов монеты за время ожидания перевода (например 50); решает Тигран | Поиска по хешу и id нет — ищем в последних записях. | Query deposit/withdraw history records: limit «default 20», максимум не указан |
| venue.rate_limit_pause_ms | площадка Phemex | *метод:* окно лимита пользователя — минута, IP-блок — 5 минут; например 60000; решает Тигран | Не слать дальше после «слишком часто» (урок 15). | Rate limits: 429 + x-ratelimit-retry-after-<group> (транспорт не читает); IP 5000/5 мин — блок 5 мин |
| venue.book_seq_strict | площадка Phemex | *метод:* не задавать до записи кадров: «да» — только если в записи sequence изменений пары идёт ровно +1 | Без записи строгая проверка могла бы рвать стакан на каждом кадре. | OrderBook message: sequence «Latest message sequence» — правило номеров не описано |
| venue.book_stale_ms | площадка Phemex | *метод:* больше 60000 (контрольный снимок раз в 60 с) — например 90000; решает Тигран | Тихая пара не должна считаться устаревшей. | OrderBook message: «Snapshot messages are published with 60-second interval» |
| venue.deposit_to_spot | площадка Phemex | *метод:* не задавать; «да» — только если первый живой депозит пришёл на Contract (поле depositTo / accountType записи) | На какой счёт зачисляется депозит, в документации не сказано. | Query deposit history records: depositTo SPOT или Contract; у адреса депозита параметра счёта нет |
| venue.balance_events | площадка Phemex | *метод:* не включать до записи кадров wallet-order при сделке и депозите: в сообщении нет причины изменения | Вид изменения выводится из событий ордеров в том же сообщении — проверить. | Wallet-Order message: wallets (весь баланс), orders; причины и хеша нет |
| venue.futures_balance_currencies | площадка Phemex | `USDT` | Панель «Балансы»: фьючерсы USDT-M и UTA по USDT. | Query Account Positions (USDT-M) и Query asset (UTA): currency обязателен |

## Зачисление по блокчейну (решение 151)

Перевод сделки на кошелёк DEX засчитывается по приходу монеты в блокчейне, не дожидаясь API биржи-отправителя. Работает при заданных venue.balance_events и transfer.credit_amount_tolerance_pct у площадки DEX.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| transfer.credit_by_chain | по площадке DEX | `да` | Монета уже в кошельке, а бот ждёт хеш вывода в API биржи. Засчитывается только приход токена монеты сделки на кошелёк сделки в ожидаемом количестве, когда других ожидаемых поступлений этого токена нет; хеш биржи сверяется фоном | Сделки 04.11.2026 (BingX → DEX 11–27 с, KuCoin → DEX 22–49 с, MEXC → DEX 58–89 с); решение 151 |

## Безопасность: вывод ключа из пароля (Argon2id)

| Настройка | Область | Советуем / метод | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| security.kdf_memory_kib | вся система | `19456` | Минимум памяти OWASP | OWASP: не меньше 19456 |
| security.kdf_iterations | вся система | `2` | Минимум проходов при этой памяти | OWASP: не меньше 2 при 19456 КиБ |
| security.kdf_parallelism | вся система | *метод:* старт 1 поток (OWASP допускает 1); больше — на многоядерном ПК | Документированного минимума нет — без ложной точности | — |
| security.password_min_len | вся система | *метод:* старт не меньше 12 символов; задаёт Тигран | Документированного минимума в проекте нет | — |

## Защита по факту продажи (решение 148)

Продажи монеты подряд хуже плана — монета уходит в ручной чёрный список (как вручную), журнал и уведомление coin.sell_slip. Не задана любая из двух — защита выключена.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| guard.sell_slip_pct | вся система | *метод:* выше допуска exec.dex_slippage_pct площадки DEX — старт: допуск плюс полпроцента, дальше по сделкам на «Сделках» | В пределах допуска факт хуже плана законно; живых продаж для замера пока мало — метод, не число | Решение 148 |
| guard.sell_slip_count | вся система | `3` | Одна-две плохие продажи бывают от движения цены за перевод; три подряд — похоже на свойство монеты (скрытый налог, ловушка ликвидности) | Ориентир (решение 148) |

## Повтор продажи, не ушедшей в сеть (решения 110 и 154)

Продажа не стоила ни денег, ни газа (отказ до отправки; или ни один канал не принял, и свой узел подтвердил, что транзакции нет) — бот сам продаёт заново через режим застрявшей монеты. Не задано — повтора нет, сразу алерт.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| stuck.unsent_retries_max | вся система | `3` | Отказы бывают разовыми (очередь агрегатора, цена сборки, отказ канала); три подряд — уже не случайность, нужен человек | Ориентир (решения 110, 154; сделки ATOM 02.11, ASTER 04.11) |
| stuck.reject_retry_pause_ms | вся система | *метод:* старт `1500` (ориентир без замера: живой случай один); дальше — по журналу `send.channel_rejected`, чем отвечают каналы | Сразу в тот же блок каналы, скорее всего, ответят так же (цена газа, лимит канала); долгая пауза — цена уходит | Ориентир (решение 154) |

## Ручное управление в Telegram (решение 156)

Каждая запись экрана «Ручное управление» приходит в Telegram с теми же кнопками (событие `manual.record` — включить на экране «Уведомления»). Пока по записям нет действия — напоминание одним сообщением со списком и кнопками. Не задано — без повторов.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| notify.manual_repeat_s | вся система | `10` | Тигран: «каждые 10 секунд … пока это всё не зачистится». Напоминание — одно сообщение на все записи (у прежнего кнопки снимаются): 6 сообщений в минуту при любом числе записей — в пределах ориентира Telegram для групп (20 в минуту) | Тигран, 04.11.2026 (решение 156); FAQ Bot API «My bot is hitting limits» |

## Повтор возврата USDT (решение 163)

Возврат выручки USDT закрытой сделки на площадку покупки не состоялся (например, удержание вывода MEXC 16021), а прежний вывод точно не ушёл — бот повторяет сам. Все повторы неудачны — запись «USDT не вернулись» на экране «Ручное управление» и в Telegram с кнопками «Перегнать» и «Перегнано вручную». Не задано — без повторов, сразу запись.

| Настройка | Область | Советуем | Обоснование | Источник |
| --- | --- | --- | --- | --- |
| usdt.return_retry_s | вся система, можно по площадке | `10` | Тигран: «через каждые 10 секунд пытаемся перевести обратно, 5 попыток». Удержание MEXC 16021 снимается, когда депозит набирает подтверждения сети, — повтор через короткую паузу успевает за ним | Тигран, 05.11.2026 (решение 163) |
| usdt.return_retries_max | вся система, можно по площадке | `5` | Тигран: «5 попыток; если все 5 неудачные — алерт в Telegram с кнопками „Перегнать“ и „Перегнано вручную“». 5 повторов по 10 с — меньше минуты до решения человека | Тигран, 05.11.2026 (решение 163) |

## Деньги и риск — вводит Тигран под капитал

Торговля не включается, пока не заданы ВСЕ `money.*` и `risk.*` (docs/SETTINGS.md). Готовых чисел здесь нет — только метод.

| Настройка | Область | Метод | 
| --- | --- | --- |
| money.min_profit_usd | вся система | Порог чистой прибыли после всех затрат; старт — заметно выше типичной стоимости сделки (газ + комиссии). Подбирает Тигран |
| money.min_profit_pct | вся система | Порог прибыли в % от вложенного — страхует мелкие сделки. Подбирает Тигран |
| money.max_per_trade_usd | вся система (можно по площадке) | Потолок вложения в одну сделку — небольшая доля капитала. Подбирает Тигран |
| money.max_per_coin_usd | вся система | Потолок вложения в одну монету одновременно. Подбирает Тигран |
| money.max_in_transit_usd | вся система | Потолок суммы во всех переводах одновременно. Подбирает Тигран |
| risk.max_daily_loss_usd | вся система | Дневной стоп: убыток за день, после которого система останавливается; старт — небольшая доля капитала. Подбирает Тигран |
| risk.min_bnb_balance | кошелёк | Запас BNB на газ — на несколько сделок и отмен; исходя из типичного газа BSC. Подбирает Тигран |
