# ARCHITECTURE — карта системы

> Обновляется при каждой готовой фиче (см. docs/CODING_STANDARDS.md → «Готовая фича»).
> Главные правила проекта — в CLAUDE.md.

## Слои

```text
┌──────────────────────────────────────────────────────────┐
│ UI (браузер)  — экраны, настройки, управление            │
└───────────────▲──────────────────────────┬───────────────┘
                │ WebSocket (живые данные)  │ REST (команды, настройки)
┌───────────────┴──────────────────────────▼───────────────┐
│ api/       — веб-сервер axum                              │
├──────────────────────────────────────────────────────────┤
│ settings/  — хранилище настроек (SQLite), применение на лету │
├──────────────────────────────────────────────────────────┤
│ core/      — ядро: справочник, цены, сигнал, расчёт,      │
│              исполнение, переводы, учёт, риск             │
│              НЕ знает о конкретных площадках              │
├──────────────────────────────────────────────────────────┤
│ gateway/   — единый вход для ядра: контракт адаптера,     │
│              маршрутизация, правила, единые ошибки/события│
├───────────────┬──────────────────────────────────────────┤
│ venues/bingx  │ venues/bsc_dex  │ (дальше: gate, mexc …)  │
│ REST + WS     │ узел BSC, пулы, свопы, кошелёк            │
└───────────────┴──────────────────────────────────────────┘
storage/ — запись сырых сообщений (zstd), сделки и переводы (SQLite)
notify/  — уведомления в Telegram: какие события слать — из настроек
secrets/ — ключи и токены только в памяти; разблокировка паролем (docs/SECURITY.md)
```

## Кто кого может вызывать

| Модуль | Может использовать | Не может использовать |
| --- | --- | --- |
| core | gateway (контракт, события, ошибки), settings, storage | venues — никогда напрямую |
| gateway | core/types (только типы), settings, storage; адаптеры получает через реестр | остальное core |
| venues/* | settings, storage, свои внутренние файлы | core, другие адаптеры |
| api | core (команды и состояние), gateway (реестр: проверка подключения, списки DEX и каналов), settings, notify | venues напрямую |
| notify | settings, storage; получает события от core и gateway | venues напрямую |
| settings | storage, gateway/errors (код «не задано») | — |
| secrets | gateway/errors | всё остальное |
| storage | gateway/rules (модель правила) | core, venues |

## Контракт адаптера (gateway/contract.rs)

Каждая площадка обязана дать эти функции. Ядро и шлюз пользуются только ими.

| Функция | Что даёт ядру |
| --- | --- |
| Справочник монет и сетей | Монеты, адрес контракта в BSC, открыт ли ввод и вывод, комиссия и минимум вывода |
| Поток цен | Событие «цена или стакан обновились»; у DEX — «пул обновился» |
| Котировка на объём | Сколько заплатим или получим за Q монет (биржа — по стакану, DEX — по пулу) |
| Купить, продать | Факт исполнения: объём, средняя цена, комиссия |
| Вывести, статус вывода | ID вывода, хеш транзакции, статус |
| Адрес депозита, статус депозита | Адрес в BSC, статус зачисления |
| Балансы | USDT, монеты, BNB на газ |
| Ошибки | Единый код ошибки + исходный ответ площадки (+ факт транзакции в сети, если она уже ушла) |
| Проверить подключение | «Подключено» или точная причина сбоя (неверный ключ, нет прав, IP не разрешён, таймаут и т.д.) |
| Стоимость вывода `withdraw_cost` | Биржа — комиссия вывода из API; DEX — газ перевода (BNB) и налог на перевод |
| Перепроверить монеты `recheck_coins` | DEX — налоги и крючки сейчас; результаты — событиями CoinCheck / PoolCheck. Биржа — `NotApplicable` |
| Виды пулов `dex_families` | DEX — DEX и виды пулов (id области настроек «DEX»). Биржа — `NotApplicable` |
| Каналы отправки `send_channels`, `check_send_channel` | DEX — список каналов и «Проверить подключение» канала. Биржа — `NotApplicable` |

Котировка (`Quote`) несёт список затрат `costs` (вид, сумма, в чём, учтено ли в цене) и, у DEX, маршрут `route`
(прямой пул или умный). Ядро добавляет свои затраты (вывод, возврат USDT, запас r) и переводит всё в USDT —
так собирается полный список затрат решения 35.

События адаптера идут в канал из `subscribe_market`: стакан, пул, «данные недостоверны», проверка налогов монеты,
проверка крючка пула, состояние канала отправки.

### Стыки (утверждает Тигран до фазы 2, docs/AGENTS.md)

| Стык | Файл |
| --- | --- |
| Общие типы ядра | src/core/types/: market.rs, pool.rs (пулы V2/V3/CL/Bin, крючки A/B/C), trade.rs (котировка с затратами и маршрутом), cost.rs, transfer.rs, check.rs (налоги, крючки, чёрный список), send.rs (каналы, транзакция в сети), control.rs (статистика газа, застрявшие монеты, ручное управление) |
| Контракт адаптера `Venue` | src/gateway/contract.rs |
| Единые коды ошибок | src/gateway/errors.rs |
| События адаптера и шлюза | src/gateway/events.rs |
| Модель правил шлюза | src/gateway/rules.rs |
| Описание настройки и области | src/settings/schema.rs, src/settings/scope.rs (области: система, площадка, монета, событие, DEX, канал) |
| Каталог настроек | src/settings/catalog/ (зеркало docs/SETTINGS.md) |
| Каталог событий уведомлений (с кнопками) | src/notify/catalog.rs |
| Известные секреты | src/secrets/mod.rs (`KNOWN`) |
| REST и WebSocket для интерфейса | src/api/server.rs (список маршрутов), src/api/README.md (в т.ч. будущие маршруты экранов DEX) |

## Поток данных одного тика

1. Адаптер получает сообщение площадки → переводит в событие формата ядра → шлюз → ядро.
2. `core/market_state.rs` обновляет цену монеты на этой площадке.
3. `core/signal.rs` пересчитывает только направления этой монеты, где участвует площадка: 2×(N−1) сравнений.
4. Прошло порог и фильтры шума → кандидат в очередь `core/profit.rs`.
5. Прибыль ≥ порога → `core/executor.rs` ведёт сделку через шлюз: покупка → перевод (`core/transfers.rs`) → продажа.
6. `core/ledger.rs` сверяет план и факт; `core/risk.rs` проверяет стоп-условия.
7. Всё, что видит ядро, уходит в интерфейс через `api/ws.rs`.

## Шаги ТЗ → файлы

| Шаг ТЗ | Файлы |
| --- | --- |
| 1. Справочник монет и направлений | core/registry.rs, core/blacklist.rs, venues/bingx/wallet.rs, venues/bsc_dex/pools/*, venues/bsc_dex/check/* |
| 2. Прослушка цен | venues/bingx/ws*.rs, venues/bsc_dex/node/*, venues/bsc_dex/pools/events_*.rs, core/market_state.rs |
| 3. Сигнал и фильтр шума | core/signal.rs |
| 4. Расчёт выгодности | core/profit.rs, core/costs.rs, core/convert.rs, venues/bsc_dex/math/*, venues/bsc_dex/quote/* |
| 5. Исполнение | core/executor.rs, core/transfers.rs, core/usdt_return.rs, core/stuck.rs, venues/bingx/trade.rs, venues/bsc_dex/exec/*, venues/bsc_dex/send/* |
| 6. Контроль и учёт | core/ledger.rs, core/gas_stats.rs, core/manual.rs, core/risk.rs |
| Правила шлюза | gateway/rules.rs |
| Команды человека (интерфейс, кнопки Telegram) | core/commands.rs, notify/updates.rs, api/* |
| Интерфейс | api/*, ui/* |

## Карта ядра (core/) — план, этапы 2–5

Каждый файл ≤ ~200 строк; ядро видит площадки только через шлюз.

| Файл | Что делает | Настройки |
| --- | --- | --- |
| types/* | Общие типы (стык) | — |
| registry.rs | Справочник: монеты биржи ∩ пулы DEX, адрес контракта совпадает, ввод/вывод BSC открыт | — |
| blacklist.rs | Чёрный список: проверка не прошла, налог выше порога, ручной; выход сам после перепроверки | tax.max_*, coin.blocked* |
| market_state.rs | Стаканы, пулы, свежесть, проверки монет и пулов из событий адаптеров | feed.stale_after_ms |
| signal.rs | Грубый спред, фильтр шума | signal.* |
| convert.rs | Перевод затрат в USDT по живым ценам (BNB, монета) | — |
| costs.rs | Полный список затрат направления (решение 35): котировки обеих сторон + вывод + возврат USDT + запас r | usdt_return.mode, profit.* |
| profit.rs | Объём, прибыль, порог | money.*, profit.* |
| executor.rs | Покупка → перевод → продажа через шлюз | exec.* |
| transfers.rs | Вывод, ожидание депозита, минимальный депозит | transfer.* |
| usdt_return.rs | Перегон USDT между биржей и кошельком (решение 34) | usdt_return.* |
| stuck.rs | Застрявшая монета: ожидание / порог убытка / перегон (решение 37) | stuck.* |
| manual.rs | «Беру на себя», «Вернуть боту» — учёт по реальному балансу | — |
| commands.rs | Команды из интерфейса и кнопок Telegram (`ManualCommand`) | — |
| ledger.rs | План против факта, сверка балансов | ledger.* |
| gas_stats.rs | Статистика газа по монете (решение 36) | — |
| risk.rs | Стоп-условия | risk.* |

## Раскладка адаптера DEX (venues/bsc_dex/) — план

Один адаптер = одна площадка `bsc_dex` (один кошелёк, один узел); внутри — DEX (PancakeSwap, Uniswap), виды пулов и каналы отправки.
Каждый файл ≤ ~200 строк, у каждой функции — ссылка на официальный источник (research/flow/bsc_*.md).

```text
src/venues/bsc_dex/
├── README.md          официальные источники, адреса, расхождения
├── mod.rs             реализация контракта Venue
├── ctx.rs             окружение: настройки (площадка, DEX, канал), секреты, журнал, сырые сообщения
├── errors.rs          ответы узла и каналов → единые коды
├── node/              rpc.rs (JSON-RPC по HTTP), ws.rs (WSS, переподключение), subs.rs (пачки подписок logs),
│                      heads.rs (newHeads, свежесть, реорги), multicall.rs, abi.rs (селекторы и topic0 со ссылками)
├── wallet/            signer.rs (ключ из секретов, подпись alloy), nonce.rs (строгая очередь, решение 45),
│                      balances.rs, approve.rs (approve, Permit2), transfer.rs (перевод на биржу)
├── pools/             discover_v2.rs, discover_v3.rs, discover_v4.rs (Initialize, PoolId), registry.rs,
│                      events_v2.rs, events_v3.rs, events_cl.rs, events_bin.rs, hooks.rs (флаги → группа A/B/C)
├── math/              v2.rs, full_math.rs, tick_math.rs, sqrt_price_math.rs, tick_bitmap.rs,
│                      swap_v3.rs, swap_cl.rs (V4: знак, композиция комиссий), bin_price.rs, bin_swap.rs, bin_fee.rs
├── quote/             direct.rs (лучший пул), smart.rs (маршрут по пулам PancakeSwap), costs.rs (затраты котировки),
│                      quoter.rs (сверка с квотером / MixedQuoter)
├── check/             taxes.rs (пробный обмен eth_call: покупка, продажа, перевод), hooks_probe.rs, cycle.rs (цикл, события)
├── exec/              v2.rs, v3.rs, v4.rs (прямой обмен), universal.rs (умный режим), build.rs (газ, проверка баланса,
│                      eth_call перед отправкой), track.rs (квитанция, откат, «не попала»)
└── send/              mod.rs (общий контракт канала, выбор, параллельная отправка), stats.rs, по файлу на канал:
                       public.rs, mev_guard.rs, club48.rs, blockrazor.rs, blockrazor_builder.rs, blink.rs (3 канала),
                       jetbuilder.rs, nodereal.rs, nodereal_api.rs, bloxroute_protect.rs, bloxroute_cloud.rs, flashblock.rs
```

Виды пулов (id области «DEX»): `pancakeswap_v2`, `pancakeswap_v3`, `pancakeswap_infinity_cl`, `pancakeswap_infinity_bin`,
`uniswap_v2`, `uniswap_v3`, `uniswap_v4`; DEX: `pancakeswap`, `uniswap`.
Каналы (id области «канал», решение 41): `public`, `pancake_mev_guard`, `club48`, `blockrazor`, `blockrazor_builder`,
`blink_merkle`, `blink_public`, `blink_private`, `jetbuilder`, `nodereal`, `nodereal_api`, `bloxroute_protect`,
`bloxroute_cloud`, `flashblock`. Список id — в коде адаптера (это перечень его модулей), адреса и ключи — настройки и секреты.

## Готовые фичи

Сюда добавляется строка при каждой готовой фиче: дата, фича, файлы, настройки, экран интерфейса.
Фича готова только со всеми пятью частями.

| Дата | Фича | Файлы | Настройки | Экран |
| --- | --- | --- | --- | --- |
| 2026-10-23 | Хранилище настроек: каталог без значений по умолчанию, проверка, SQLite с историей, изменения на лету | src/settings/*, src/storage/db.rs | все из docs/SETTINGS.md | экран «Настройки» |
| 2026-10-23 | Журнал: запись в SQLite и живой поток в интерфейс | src/storage/journal.rs, src/api/journal.rs, src/api/ws.rs | — | экран «Журнал» |
| 2026-10-23 | Правила шлюза: модель, хранение, выбор правила (площадка сильнее общего) | src/gateway/rules.rs, src/storage/rules.rs, src/api/rules.rs, src/api/rules_meta.rs | — (правила вводятся в интерфейсе) | экран «Правила шлюза» |
| 2026-10-23 | Уведомления в Telegram: события по каталогу, вкл/выкл и площадки по каждому, «Проверить подключение» (getMe + getChat) | src/notify/* , src/api/check.rs | notify.*, http.timeout_ms; секрет telegram.bot_token | экран «Уведомления» |
| 2026-10-23 | Секреты: зашифрованный файл (Argon2id + XChaCha20-Poly1305), разблокировка паролем, ключи только в памяти | src/secrets/*, src/api/secrets.rs | security.* | экран «Разблокировка»; ключи площадок — на экране «Площадки» |
| 2026-10-23 | Защита веб-сервера от чужих страниц (Host/Origin) | src/api/guard.rs | аргументы --listen, --allow-origin | — |
| 2026-10-23 | Адаптер BingX (спот): справочник монет и сетей, стаканы по WebSocket, котировка на объём, ордер, вывод, депозит, балансы, «Проверить подключение» | src/venues/bingx/*, src/storage/raw_log.rs | venue.* (bingx), raw.*, feed.stale_after_ms; секреты bingx.api_key, bingx.api_secret | экран «Площадки»: проверка подключения, ключи, настройки площадки (статусы соединений — когда бэкенд начнёт их отдавать) |
| 2026-10-23 | Веб-интерфейс (Svelte 5 + Vite): экраны «Разблокировка», «Площадки», «Настройки», «Правила шлюза», «Уведомления», «Журнал»; живые обновления через WebSocket; раздача собранного интерфейса бэкендом | ui/*, src/api/venues.rs, src/api/rules_meta.rs, src/api/static_ui.rs, src/api/server.rs, src/cli.rs | — (аргумент запуска --ui-dir) | все перечисленные |
| 2026-10-23 | Интерфейс: области настроек «DEX» и «канал» (id — от адаптера DEX), виды значений «выбор из списка», «адрес BSC», верхняя граница целого; на «Площадках» — виды пулов DEX, ключи и «Проверить подключение» каналов, ключи площадок, ещё не подключённых в шлюзе | src/api/dex.rs, src/api/check.rs, src/api/state.rs, ui/src/lib/dex.svelte.js, ui/src/components/DexFamilies.svelte, VenueChannels.svelte, SettingEditor.svelte, SettingCard.svelte | dex.*, smart.*, channel.* (без новых); секреты bsc_dex.wallet_key, bsc.node_*_key, channel.<id>.key | «Площадки», «Настройки» |
| 2026-10-23 | Интерфейс: экраны «Монеты» (налоги, пулы и крючки, чёрные списки, «Перепроверить», ручной чёрный список), «Ручное управление», «Каналы отправки» (настройки, ключ, проверка, статистика), «Статистика газа»; данные ядра — честное «нет данных» (503), пока ядро не подключено к api | src/api/coins.rs, src/api/control.rs, ui/src/pages/Coins.svelte, Manual.svelte, Channels.svelte, GasStats.svelte, ui/src/components/CoinRow.svelte, ManualBlacklist.svelte, ChannelCard.svelte, NoData.svelte, ui/src/lib/remote.svelte.js | coin.blocked, coin.blocked_note, channel.* (без новых) | «Монеты», «Ручное управление», «Каналы отправки», «Статистика газа» |
| 2026-10-23 | Кнопки Telegram: сообщения с inline-кнопками (stuck.loss_alert), приём нажатий getUpdates длинным опросом только из настроенного чата, ответ на нажатие, правка сообщения, команда ядру `ManualCommand` | src/notify/updates.rs, press.rs, callback.rs, tg_buttons.rs, tg_text.rs, dispatch.rs | notify.updates_timeout_s, notify.telegram_chat, notify.telegram_api_url, http.timeout_ms; секрет telegram.bot_token | экран «Уведомления» (настройки notify.*); подключение к ядру и сигнал wake — в main.rs/api (техлид) |
| 2026-10-23 | Ядро, часть 1: справочник монет и направлений (первичный фильтр с причинами отсева), чёрный список (ручной, проверка налогов, налог выше порога) и допуск пулов с крючками, состояние рынка из событий адаптеров, подписки площадок, грубый спред и фильтр шума; уведомления venue.transfer_status, coin.blacklisted, coin.unblacklisted, coin.check_done, pool.hook_excluded | src/core/* (кроме types), src/main.rs, venues/bingx/wallet.rs (depositMin) | venue.enabled, venue.status_poll_ms, venue.quote_asset, money.max_per_trade_usd, signal.*, tax.max_*, hooks.*, coin.blocked* | данные для экранов «Монеты и направления» (Core::coins_json, формат GET /api/coins) и «Живой экран» (Core::spreads, поток Core::subscribe) готовы; маршруты и экраны — агент интерфейса (Issue #9) |
| 2026-10-23 | Ядро, часть 2: полный расчёт кандидата (решение 35): котировки обеих сторон, стоимость вывода, минимумы вывода и минимальный депозит (неизвестен — сделки нет), доля глубины B (h), возврат USDT в каждой сделке, запас r = k·σ·√T (σ и время блока ядро меряет само), газ в USDT по живому стакану BNB; подбор объёма по уровням стаканов в пределах бюджета; порог прибыли в USDT и %; отдельная очередь расчёта; уведомление deal.candidate | src/core/convert.rs, costs.rs, dirctx.rs, limits.rs, profit.rs, candidates.rs, vol.rs | money.*, profit.price_move_k, profit.liquidity_share_h, profit.default_transit_time_s, usdt_return.mode, venue.quote_asset | «Живой экран»: событие candidate (расчёт и причина отказа) в потоке Core::subscribe; экран — агент интерфейса (Issue #9) |
| 2026-10-23 | Ядро, часть 3: исполнение сделки (предпроверка, покупка, перевод, продажа, сверка плана и факта), правила шлюза к ошибкам, застрявшие монеты (три режима), ручное управление и команды (интерфейс, Telegram из настроенного чата), возврат USDT, учёт сделок в SQLite, статистика газа, сверка балансов, стоп-условия и режим системы | src/core/{deal,persist,risk,rules_apply,precheck,trade_ops,fills,executor,transfers,transfer_watch,sell,stuck,relay,manual,manual_return,commands,usdt_return,ledger,gas_stats,reconcile,inner}.rs | exec.dex_slippage_pct, transfer.max_time_s, ledger.reconcile_interval_s, risk.*, stuck.*, usdt_return.*, signal.route_cooldown_s, notify.telegram_chat | данные и команды для экранов «Сделки» (Core::deals), «Балансы» (поток balances), «Ручное управление» (Core::manual, Core::command), «Статистика газа» (Core::gas_stats), «Управление» (Core::control, set_mode, resume_venue, release_quarantine); маршруты и экраны — агент интерфейса (Issue #9), кнопки Telegram — Issue #10 |
| 2026-10-23 | Ядро: решения 47–50 — «невыгодно» = итог ниже нуля; частичная покупка — больший итог из «довезти» и «продать на A»; σ по окну N минут; время в пути «вручную» / «вручную + авто» с историей переводов в SQLite (переживает перезапуск); таблица core_state — в схеме storage/db.rs | src/core/{vol,transit,partial,sell,stuck,executor,dirctx,persist,ledger}.rs, src/storage/db.rs | profit.sigma_window_min, profit.transit_mode, profit.transit_auto_after, profit.default_transit_time_s | экран «Настройки» (новые настройки — по каталогу) |
| 2026-10-23 | Сведение ядра, интерфейса и Telegram: Core в AppState, маршруты экранов ядра с реальными данными (монеты, ручное управление, статистика газа и каналов, спреды, кандидаты, сделки, время в пути, управление «Старт/Стоп/Пауза»), события ядра в /api/ws; кнопки Telegram → Core::command (CommandTx в стыке, поле by, UUID алерта, по закрытому алерту — игнор), wake после изменения секретов, пауза повтора getUpdates; стыки: dex_id в схеме настроек, SendMethod::CHOICES, recheck по Asset, pools[].block — номер блока | src/api/{state,coins,control,core_api,dex,ws,server,secrets}.rs, src/core/{commands,manual,candidates,ids}.rs, src/core/types/{control,send}.rs, src/settings/{schema,catalog/*}.rs, src/notify/{press,updates}.rs, src/main.rs, ui/* | notify.updates_retry_ms | «Живой экран», «Сделки», плашка режима на всех экранах; «Монеты», «Ручное управление», «Статистика газа», «Каналы отправки» — с данными ядра (снимки docs/screens/) |

## Открытые вопросы

| Вопрос | Где решается |
| --- | --- |
| Авторизация доступа к веб-серверу. До решения — запуск только на 127.0.0.1; чужие Host/Origin отклоняются (api/guard.rs) | Тигран |
| Формат условия правила и условия события уведомлений | Этап 5 |
| Вопросы по стыкам DEX (минимальный депозит BingX, источник цены BNB, длинный опрос Telegram и др.) | Тигран, отчёт PR «Стыки DEX и ядра» |
