# api — веб-сервер для интерфейса (axum)

| Маршрут | Что делает |
| --- | --- |
| `GET /api/settings/schema` | Каталог настроек и событий уведомлений |
| `GET /api/settings` | Все заданные значения |
| `PUT /api/settings` | `{name, scope, value}`; `value: null` — очистить |
| `GET /api/settings/history?limit=N[&name=]` | История изменений |
| `GET /api/settings/export[?secrets=true]` | Экспорт всех заданных настроек одним файлом TOML (имя+область+значение, в шапке версия формата); `secrets=true` — полный бэкап: секция `[secrets]` с ЗАШИФРОВАННЫМ хранилищем секретов как есть (base64, без расшифровки), имя файла `arb-backup.toml`; `secrets=true` без файла секретов — 400 (api/settings_io.rs) |
| `POST /api/settings/import` | Импорт настроек из TOML (чистые настройки или полный бэкап): проверка по схеме и применение поштучно через `Store::set` (автор «import» — журнал и обновление на лету); секция `[secrets]` ставит зашифрованное хранилище и блокирует его. Тело — текст TOML. Ответ `{applied:[{name,scope}], failed:[{name,scope,reason}], secrets_installed, secrets_error?}`; 400 — тело не TOML; 500 — сбой базы |
| `GET /api/first-run` | Таблица первого запуска — советуемые стартовые значения (справочные данные, не автозапись; api/first_run.rs) |
| `GET /api/rules`, `POST /api/rules`, `PUT /api/rules/{id}` | Правила шлюза |
| `GET /api/rules/meta` | Справочник для формы правил: коды ошибок, действия с параметрами, виды области |
| `GET /api/venues` | Площадки, зарегистрированные в шлюзе (id по алфавиту) |
| `GET /api/venues/kinds` | Площадки с видом `[{id, kind: "Cex" \| "Dex"}]` — подпись направления перевода у настроек `venue_role: "TransferSource"` (решение 51) |
| `GET /api/venues/{id}/dex-families` | DEX и виды пулов адаптера (id области «DEX»); биржа — 400 «не применимо» |
| `GET /api/venues/{id}/dex-cycles` | Сигналы циклов DEX ↔ DEX (решение 85): `[DexCycle {venue, base, base_ticker, input, output, gross_profit, gas_units, gas_bnb, tip_bnb, bnb_cost, bnb_cost_in_base, net_profit, hops: [{index, family, kind, pool, token_in, token_out, fee_pct, hook_pct, gas_units}], executable, exec_note, block}]` — шлюз `dex_cycles`; за `dex_dex.enabled`, выключено — пусто; биржа — 400 «не применимо» |
| `GET /api/venues/{id}/pool-directory` | Справочник пулов от индексатора (решение 58): `PoolDirectory {source, network, attribution_text, attribution_url, dexes: [{id, name}], dexes_at, tokens: [{token, ticker, fetched_at, pages, total, error}], pools: [{token, dex_id, pool, base, quote, base_symbol, quote_symbol, reserve_usd, status}], error}`; `status` — `"Unchecked"`, `"Taken"`, `{"NotConnected": причина}`, `{"Rejected": причина}`, `{"Unsupported": причина}` (например нативный BNB); биржа — 400; с решения 121 — `tokens[].note` («индексатор не знает пулов монеты», 404 — сведение, не ошибка) и `factory {enabled, at, calls, found, taken, failed, notes, error}` — поиск по фабрикам на своём узле (`index.factory_scan`) |
| `POST /api/dex/dry-swap` | «Сухая сделка DEX» (решение 121, C4): тело `{venue?, asset: {ticker, bsc_contract}, side: "Buy"\|"Sell", usdt: "100"}` → `DexDrySwap {venue, asset, side, usdt, qty, form, pools: [{dex, kind, pool, leg}], out_symbol, quote_out, live_out, min_out, sim_out, gas_swap, gas_approve, approvals, block, funding, ok, reason, notes}` — шлюз `dex_dry_swap`: сборка своим сборщиком как для сделки и eth_simulateV1 от кошелька, без отправки. `venue` не задан — единственная DEX реестра. Отказ сборки/симуляции — `ok: false` и `reason` (HTTP 200); 400 — нет адреса монеты, сумма ≤ 0, площадка не DEX |
| `POST /api/dex/dry-cycle` | «Сухая сделка DEX», режим «цикл» (решение 124): тело `{cycle: DexCycle, budget_usdt?, executor?}` → `DexDrySwap`; весь путь ядра (бюджет, размер, граница — core/cycle_dry.rs) и симуляция у адаптера, ничего не отправляется; план не составлен — 400 с причиной |
| `GET /api/venues/{id}/executor/quote` | Решение 122 (часть X): оценка развёртывания своего исполнителя ArbExecutor — `ExecutorDeployQuote {venue, wallet, nonce, address, gas, gas_price_wei, cost_bnb, cost_usdt, bytecode_sha256, solc, bytecode_len, note}` (шлюз `dex_executor_quote`), ничего не отправляет; биржа — 400 |
| `POST /api/venues/{id}/executor/deploy` | Развёртывание исполнителя с кошелька бота по подтверждению человека: тело `{sha256}` — хеш байткода из оценки → `ExecutorDeployed {venue, address, chain, code_ok, text}` (шлюз `dex_executor_deploy`); хеш не тот — 502 QuoteMismatch; биржа — 400. В «Сухой сделке» `POST /api/dex/dry-swap` поле `executor: true` — режим «через исполнитель» (код подставлен в симуляцию) |
| `GET /api/channels/{venue}` | Каналы отправки адаптера DEX (id области «канал»): `[{info, stats}]`; `stats: null` — по каналу ещё не было событий |
| `GET /api/wallet-queue/{venue}` | Очередь транзакций кошелька DEX: свободна / ждёт итога (nonce, хеш) / остановлена до ручной проверки (причина) |
| `POST /api/wallet-queue/{venue}/release` | «Снять очередь после ручной проверки» `{pending_ok}` — только остановленную (иначе 400); nonce ещё не занят без `pending_ok` — 409, интерфейс просит второе подтверждение |
| `GET /api/wallets` и др. (решение 132) | Экран «Маппинг кошельков»: `GET /api/wallets` → `{unlocked, dex, wallets[{id, name, secret, executor, address, address_error, rows, open_deals, legacy}], rows[{row, kind: cex/dex_dex/probe, wallet}], shared[{wallet, name, rows}]}`; `POST /api/wallets {name, key, password}` (секреты разблокированы; ключ — в зашифрованный файл под `wallet.<id>.key`, ответ без ключа; ключ того же адреса, что у другого кошелька, — 400); `PATCH /api/wallets/{id} {name}`; `DELETE /api/wallets/{id}` (400 — в маппинге или в незавершённой сделке; секрет не удаляется); `PUT /api/wallets/map {row, wallet|null}` (строки — биржи реестра, `dex_dex`, `probe`); `GET /api/wallets/history?limit=N`; `GET /api/wallets/{id}/balances` → `{bnb, usdt, quote, error}`; `PUT /api/wallets/{id}/executor {address|null}` (запись — только после проверки «код совпал, владелец — этот кошелёк»); `POST /api/check/wallet/{id}` → ConnectionCheck. Исполнитель, очередь, проверка исполнителя — те же маршруты с `?wallet=<id>` (`/api/venues/{id}/executor/quote|deploy`, `/api/wallet-queue/{venue}[/release]`, `/api/check/executor/{venue}`); развёртывание с кодом, совпавшим в сети, записывает адрес у кошелька. Ключ не отдаётся никогда — только имя секрета и адрес |
| `GET /api/journal?limit=N[&before=ID]` | Журнал |
| `GET /api/coins`, `POST /api/coins/recheck` | Экран «Монеты»: справочник из ядра; «Перепроверить» — шлюз `recheck_coins` |
| `GET /api/manual`, `POST /api/manual/command` | Экран «Ручное управление»: алерты и монеты человека, команды ядру |
| `GET /api/gas-stats` | Экран «Статистика газа» |
| `GET /api/spreads`, `GET /api/candidates` | «Живой экран»: спреды направлений и последние расчёты кандидатов с причиной отказа |
| `GET /api/episodes`, `GET /api/episodes/{id}`, `GET /api/episodes/{id}/file` | «Эпизоды» (решение 74): список, записи эпизода, файл .jsonl.zst |
| `GET /api/windows`, `GET /api/windows/{id}`, `GET /api/windows/{id}/file` | «Окна» (решение 125): страница списка с фильтрами, кадры окна разобранные адаптером, файл .jsonl.zst |
| `GET /api/uncapped/summary`, `GET /api/uncapped`, `GET /api/uncapped/{id}`, `GET /api/uncapped/deal/{id}` | «Упущено из-за потолка» (решение 141): итоги за период, записи страницей, одна запись, запись сделки (api/uncapped.rs) |
| `GET /api/deals` | Экран «Сделки»: план и факт по шагам |
| `GET /api/transits` | История времени в пути (решение 50) |
| `GET /api/control`, `POST /api/control`, `POST /api/control/venue/{id}/resume`, `POST /api/control/quarantine/{id}/release`, `POST /api/control/daily-limit/reset`, `POST /api/control/venue/{id}/daily-limit/reset` | Управление: режим «Старт/Стоп/Пауза», снятие паузы площадки и карантина монеты, «Сбросить лимит дня» общий (решение 153) и по площадке (решение 173) |
| `POST /api/check/telegram` | «Проверить подключение» Telegram |
| `POST /api/check/venue/{id}` | «Проверить подключение» площадки |
| `POST /api/check/channel/{venue}/{id}` | «Проверить подключение» канала отправки (шлюз `check_send_channel`) |
| `GET /api/secrets/known` | Известные секреты: имя, площадка, описание |
| `GET /api/secrets/status` | Создан ли файл, разблокировано ли, имена секретов (без значений) |
| `POST /api/secrets/create`, `/unlock`, `/lock`, `/password` | Создать файл, разблокировать, заблокировать, сменить пароль |
| `PUT /api/secrets/{name}`, `DELETE /api/secrets/{name}` | Добавить/заменить, удалить секрет (нужен пароль) |
| `GET /api/ws` | Живой поток: `settings`, `journal`, `lagged` и события ядра (core/view.rs) |
| `GET /api/{неизвестно}` | 404 `{"error": …}` |
| `/` и всё остальное | Собранный интерфейс из `--ui-dir` (нет файла — `index.html`); без `--ui-dir` — не раздаётся |

Ответ ошибки: `{"error": "понятная причина"}`. «Нет данных» (источник ещё не подключён) — 503 с причиной, а не пустой список. Ошибка шлюза (`ApiError::gateway`): `NotApplicable` — 400,
площадка не подключена (`VenueNotConnected`) — 404, остальные коды — 502; текст — «площадка (код): сообщение». Чужие Host/Origin отклоняются (`guard.rs`). Авторизации пока нет — запускать только на 127.0.0.1 (открытый вопрос).

## Маршруты экранов DEX и ядра (контракт)

Реализованы в `dex.rs`, `coins.rs`, `control.rs`, `core_api.rs`, `check.rs`. Данные ядра — через `AppState.core`
(`arb::core::Core`); живые события ядра — в `/api/ws` из `Core::subscribe()`.

Типы — из `src/core/types/` и `src/gateway/contract.rs` (JSON — как их serde). Ошибка — `{"error": …}`.
Данные приходят из ядра (оно хранит последние события адаптеров) и из реестра шлюза; к адаптерам напрямую api не ходит.
Настройки этих экранов меняются обычным `PUT /api/settings` (области `coin`, `dex`, `channel`).
Область «DEX»: чей id у настройки — поле схемы `dex_id` (`"Dex"` — имя DEX, `"Family"` — вид пула, `"None"`). Необязательная настройка — поле схемы `optional: true` (решение 73, например `dex.depth_log_min_s`): не задана — функция работает, отключено только описанное; в счётчик «не задано» не входит. Ограничения `limit`: кроме прежних — `"PercentOpen"` (больше 0 и меньше 100) и `"PercentBelow100"` (от 0 и меньше 100); значение вне ограничения при сохранении — ошибка с причиной.
Способ отправки канала ↔ вариант `channel.method`: `SendMethod::CHOICES` / `setting_value()` (core/types/send.rs).

### Экран «Монеты» (налоги, крючки, чёрные списки)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/coins` | — | `[{asset: Asset, venues: [VenueId], taxes: CheckState<TokenTaxes>, tax_source, checked_at, block: CoinBlock \| null, pools: [{venue, family, kind: PoolKind, pool: PoolRef, fee_pips, hooks: HookInfo \| null, check: PoolCheck \| null, pair: String, admission: String \| null, block: u64, received_at}], via: [{venue, via}], checks: [CoinCheck], problem: String \| null, presence: [Presence], directions: [Direction]}]` — `pools[].block` — номер блока, по которому получено состояние пула; `admission` — причина исключения пула (крючок, проверка), null — пул допущен; `pair` — второй токен пула (USDT или промежуточная монета); `via` — через какую промежуточную монету допущена монета без пула с USDT (решение 52); `pools[].depth: PoolDepth | null` — что о пуле знает адаптер DEX (решение 73, шлюз `pool_depths`): `buy_usd`, `sell_usd` — глубина покупки и продажи в полосе `band_pct` (USDT), `min_usd` — порог, `deep_buy`, `deep_sell` — вердикт с гистерезисом, `full` — полоса прочитана целиком, `depth_error` — почему не посчитана, `asleep` — слушается только добавление ликвидности, `valid`, `cross_check_buy`, `cross_check_sell: {state: "not_checked" | "ok" | "failed", reason}` — сверка расчёта по каждой стороне (у DEX — квотер; стороны независимо, решение 73), `details: [String]` — подробности площадки строками (у DEX — «tickSpacing N»); `pools[].depth_unavailable` — почему адаптер не ответил |
| `POST /api/coins/recheck` | `{venue: VenueId, assets: [Asset]}` (тикер + адрес контракта: одинаковый тикер бывает разными токенами) | `{ok, details}` — шлюз `recheck_coins`; биржа отвечает «не применимо» |
| ручной чёрный список | `PUT /api/settings` `{name:"coin.blocked", scope:{kind:"Coin",id:"CAKE"}, value:true}` (+ `coin.blocked_note`) | как у настроек |
| WS `/api/ws` | — | `{"type":"coin_check","data":CoinCheck}`, `{"type":"pool_check","data":PoolCheck}`, `{"type":"coin_block","data":{asset, block: CoinBlock \| null}}` (null — вышла из списка) |

### Экран «Ручное управление» (решение 37)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/manual` | — | `{alerts: [StuckAlert], holdings: [ManualHolding], halted: [Deal], records: [ManualRecord]}` (halted — остановленные сделки, решение 100; records — записи с кнопками `{key, kind, object_id, deal_id, venue, text, buttons: [{code, title, action, confirm, input, disabled, quiet}]}`, решение 156: экран рисует кнопки из них, те же уходят в Telegram) |
| `POST /api/manual/command` | `{action: ManualAction}` — `{"SellNow":{"alert_id":…}}`, `{"TakeOver":{"alert_id":…}}`, `{"ReturnToBot":{"holding_id":…}}`, `{"SellHalted":{"deal_id":…}}`, `{"SellNowReturn":{"alert_id":…}}`, `{"SellHaltedReturn":{"deal_id":…}}` (решение 101), `{"SoldManually":{"deal_id":…,"usdt":"24.44"}}`; решение 157 (из Telegram): `{"SetMode":{"mode":"Running"}}`, `{"ResumeVenue":{"venue":…}}`, `{"ReleaseQuarantine":{"id":…}}`, `"ResetDailyLimit"` | `{ok, details}`; источник — `CommandSource::Ui`, пишется в журнал; id алерта — UUID; по закрытому алерту — ok и «проигнорирована» |
| WS `/api/ws` | — | `{"type":"stuck_alert","data":StuckAlert}`, `{"type":"manual","data":{alerts, holdings}}`, `{"type":"command","data":{action, who, ok, text}}` (итог команды человека, решение 156) |

### Экран «Каналы отправки» (решения 33, 41–44)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/channels/{venue}` | — | `[{info: SendChannelInfo, stats: SendChannelStats \| null}]` — шлюз `send_channels` + последние события ядра; биржа — 400 «не применимо» |
| `GET /api/wallet-queue/{venue}` | — | `WalletQueue {venue, state: Free \| Busy{nonce, hash, since} \| Halted{reason, nonce, hash, since}}` — шлюз `wallet_queue`; биржа — 400 |
| `POST /api/wallet-queue/{venue}/release` | `{pending_ok: bool}` | `WalletQueue` после снятия — шлюз `release_wallet_queue`; очередь не остановлена — 400; nonce не занят без `pending_ok` — 409 |
| `/api/wallets*` | см. строку «решение 132» выше | Ответы без ключей: только имя секрета и адрес (тест tests/api_wallets.rs) |
| `POST /api/check/channel/{venue}/{id}` | — | `{ok, details}` — шлюз `check_send_channel`, пишется в журнал |
| настройки канала | `PUT /api/settings` с `scope:{kind:"Channel",id:"club48"}` (`channel.*`); ключ — `PUT /api/secrets/channel.<id>.key` | как у настроек и секретов |
| WS `/api/ws` | — | `{"type":"send_channel","data":SendChannelStats}` |

### Экран «Статистика газа» (решение 36)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/gas-stats` | — | `[CoinGasStats]`, по монетам |
| WS `/api/ws` | — | `{"type":"gas_stats","data":CoinGasStats}` |

### «Живой экран», «Сделки», «Управление» (ядро)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/spreads` | — | `[SpreadView]`: `{direction, prices, spread_pct, state, above_since, updates, at}`; `state` — `"Below"`, `"Holding"`, `"Candidate"`, `"Anomaly"`, `{"NoPrice": причина}`, `{"NotConfigured": причина}`, `{"Cooldown": {until}}` |
| `GET /api/dry` | — | Сухой режим для плашки (решение 74, core/dry.rs): `{state, budget \| null, reason \| null}`; `state` — `on` (сухой, бюджет), `nobudget` (сухой без dry.budget_usdt), `broken` (сухой: значение dry.enabled не читается), `off`, `unset` (обычный); перечитывать по сообщению `settings` с именем `dry.*` |
| `GET /api/episodes?limit=N` | — | `{episodes: [EpisodeInfo], off: причина \| null, used_bytes}`; `EpisodeInfo` — `{id, direction, dry, started_at, ended_at \| null, max_profit, max_profit_unbounded \| null, records, close_reason \| null, stopped \| null}` (решение 74, core/episode.rs); живое сообщение `episode` — та же сводка |
| `GET /api/episodes/{id}?limit=N` | — | `{info, records: [JSON], truncated}`: записи файла — `{"type": "open", info, card}`, `{"type": "calc", t, qty, profit_usd, profit_pct, buy_avg, sell_avg, books: [{venue, exchange_time, received_at, bids, asks}], pools: [Pool], card}`, `{"type": "close", info}`; 404 — нет эпизода |
| `GET /api/episodes/{id}/file` | — | файл `<id>.jsonl.zst` (application/zstd, attachment); 400 — неверный id, 404 — файл удалён |
| `GET /api/windows?limit=N&offset=M&coin=T&deal=yes\|no&outcome=plus\|minus\|zero` | — | `{windows: [WindowInfo], total, off: причина \| null, used_bytes}`; `WindowInfo` — `{id, asset, dry, directions, venues: [{venue, asset, since, tap: {Ok: {book_channel, trades_channel: {Ok\|Err}, trades_form}} \| {Err}}], started_at, ended_at \| null, max_profit, max_profit_unbounded, deals: [{id, open, step, result_usd}], outcome, frames, lost, bytes, stopped, close_reason}` |
| `GET /api/windows/{id}?limit=N&offset=M&levels=K` | — | у кадров изменений и снимка (stream Diff\|Snapshot) — `decoded.Ok` сводкой `{exchange_ms, full, from, to, changed}` и `book: {ok, note, version, depth, bids, asks}` (стакан всей глубины после кадра, K верхних уровней); остальное: | `{info, lines, frames: [{i, venue, stream: Book\|Trades, conn, recv_ns, bytes, decoded: {Ok: {view: {Book: {bids, asks}} \| {Trades: [{id, price, qty, taker, time_ms}]} \| {Other}, exchange_ms}} \| {Err}}], frames_read, truncated, deals: [{id, direction, step, result_usd, timeline}]}`; строки файла — `{"type": "open"\|"tap"\|"join"\|"close"}` и кадры `{"type": "f", v, s, c, ns, t \| h}` (t — текст, h — hex двоичного); 404 — нет окна |
| `GET /api/windows/{id}/file` | — | файл `<id>.jsonl.zst` (application/zstd, attachment); 400 — неверный id, 404 — файл удалён |
| `GET /api/delistings` | — | Решение 175: `[{venue, got: {items: [{ticker, symbol, trading_end \| null, withdraw_end \| null, source}], unknown?} \| null, at \| null, error \| null}]` — делистинги каждой биржи (core/delist.rs); `unknown` — у биржи нет источника в официальном API (не блокирует), `got = null` — ещё не получено |
| `GET /api/candidates` | — | `[{direction, verdict, at, dry?, card?}]` (у прошедшего — `dry` и карточка сигнала `card` решения 74, core/card.rs: `{dry, ticker, contract, buy_on, sell_on, qty, buy_avg, sell_qty, sell_avg, budget, profit_usd, profit_pct, books: [BookSide], pools: [PoolSide], max: {"Ok": MaxVolume} \| {"Err": причина}, links: [{venue, kind: "pair" \| "pool", label, url \| null, why \| null}]}`), новые сверху; `verdict` — `{"Accept": Calc}` или `{"Reject": {reason, best: Calc \| null}}`; `Calc.costs[]` — `{kind, stage, amount, asset, usdt \| null, in_price, in_qty, step: {index, from, to, from_token, to_token} \| null}` |
| `GET /api/deals` | — | `[Deal]` (core/deal.rs): шаг, где монеты, вложено, выручка, затраты, план, факт покупки/переводов/продаж, хронология, итог |
| `GET /api/transits` | — | `[{asset, from, to, count, avg_s}]` |
| `GET /api/control` | — | `Risk`: `{mode: "Stopped" \| "Running" \| "Paused", reason, venue_paused, errors_in_row, quarantine: [{id, ticker, label, reason}], day: {date, result_usd, final_usd, provisional_usd, provisional: {сделка: [сумма, день учёта]}, unpriced: [сделка], limit_reset: {at, base_usd, by} \| null, limit: {max_usd, loss_usd, exceeded} \| null, venues: {площадка: {result_usd, final_usd, provisional_usd, limit_reset, limit, paused}}, deal_venues: {сделка: [площадки]}}, daily_stop, cycle_paused: {DEX: причина}}` — `cycle_paused`: на паузе только циклы DEX ↔ DEX по убытку дня DEX (CEX ↔ DEX торгуют); снимает `POST /api/control/venue/{DEX}/resume` или сброс лимита дня — решение 173: `limit` — общий лимит (risk.max_daily_loss_total_usd), `venues` — итог и лимит дня по площадкам (risk.max_daily_loss_usd) — итог дня: факт + оценка позиций (решения 53, 152: стоимость монет по рынку − вложено; `unpriced` — цены не было, взят худший случай), лимит дня и его сброс (решение 153); `daily_stop` — остановлена именно по лимиту дня |
| `POST /api/control` | `{mode}` | `Risk`; пишется в журнал, уведомления `system.started` / `system.stopped` |
| `POST /api/control/venue/{id}/resume` | — | `Risk` — пауза площадки снята |
| `POST /api/control/quarantine/{id}/release` | — | `Risk` — карантин монеты снят (id — адрес контракта или тикер монеты без адреса, решение 118), `route.quarantine_out` |
| `POST /api/control/venue/{id}/daily-limit/reset` | — | `Risk` — «Сбросить лимит дня» площадки (решение 173): отметка её итога дня, её пауза по убытку дня снимается; остальные площадки и общий лимит не меняются; журнал `core.loss_limit_reset`, уведомление `risk.daily_limit_reset` |
| `POST /api/control/daily-limit/reset` | — | `Risk` — «Сбросить лимит дня» общий (решения 153, 173: вся система и все площадки, паузы площадок по убытку снимаются): отметка времени и итога дня; лимит дальше считает только изменения после неё; итог дня и сделки не меняются; режим не меняется; журнал `core.loss_limit_reset`, уведомление `risk.daily_limit_reset` |
| WS `/api/ws` | — | `spread` (SpreadView), `candidate`, `deal` (Deal), `balances` (`{venues, in_transit_usd, per_coin_usd}`), `control` (Risk), `coins` (`{coins, directions}` — перечитать GET /api/coins) |

### DEX и виды пулов (экран «Площадки», настройки области «DEX»)

«Проверить подключение» узла BSC и кошелька — существующий `POST /api/check/venue/bsc_dex` (адаптер DEX, `check_connection`).

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/venues/{id}/dex-families` | — | `[DexFamily]` — шлюз `dex_families`; биржа — 400 «не применимо» |
| `GET /api/venues/{id}/dex-cycles` | — | `[DexCycle]` — шлюз `dex_cycles` (сигналы циклов DEX ↔ DEX, решение 85; за `dex_dex.enabled`, выключено — пусто); биржа — 400 «не применимо» |
| `POST /api/dex/dry-swap` | `{venue?, asset, side, usdt}` | `DexDrySwap` — шлюз `dex_dry_swap` (решение 121, C4); биржа — 400 «не применимо» |

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Список модулей |
| `server.rs` | Маршруты и запуск; раздача интерфейса, если задан `--ui-dir` |
| `state.rs` | Общее состояние, формат ошибки, перевод ошибки шлюза в HTTP-код |
| `guard.rs` | Защита Host/Origin (для API и для страниц интерфейса) |
| `settings.rs` | Схема, значения, изменение, история настроек |
| `rules.rs` | Правила шлюза |
| `rules_meta.rs` | Справочник для формы правил (полнота проверяется при сборке) |
| `rules_codes.rs` | Коды ошибок шлюза для формы правил — смысл по-русски |
| `wallet_queue.rs` | Очередь кошелька DEX: состояние (и занят ли nonce остановленной), снятие после ручной проверки |
| `wallets.rs` | Решение 132: «Маппинг кошельков» — список, добавить (ключ сразу в зашифрованные секреты), переименовать, удалить, строка маппинга, история |
| `wallets_view.rs` | Решение 132: данные экрана — кошельки с адресами от адаптера, строки (биржи реестра, `dex_dex`, `probe`), «деньги смешиваются» |
| `wallets_dex.rs` | Решение 132: DEX на кошелёк (`venue_as`, `dex_for`), балансы BNB/USDT, «Проверить подключение» кошелька, запись исполнителя после проверки |
| `balances.rs` | Решение 144: панель «Балансы» — список бирж и ui.balances_refresh_ms (`GET /api/balances`), балансы всех счетов биржи (`GET /api/balances/{id}`, контракт account_balances; ответ кладётся в память ядра для Telegram, решение 176) |
| `wallets_migrate.rs` | Решение 132: перенос прежнего кошелька после разблокировки секретов и после ввода секрета |
| `venues.rs` | Список площадок из реестра шлюза |
| `dex.rs` | DEX и виды пулов, каналы отправки — из реестра шлюза |
| `dex_dry.rs` | «Сухая сделка DEX» (решение 121, C4): POST /api/dex/dry-swap → шлюз `dex_dry_swap` |
| `coins.rs` | Экран «Монеты»: справочник из ядра, «Перепроверить» по Asset |
| `control.rs` | «Ручное управление» и «Статистика газа» — данные и команды ядра |
| `core_api.rs` | «Живой экран», «Сделки», время в пути, «Управление» (Старт/Стоп/Пауза) |
| `journal.rs` | Журнал |
| `check.rs` | «Проверить подключение»: Telegram (и чат каждой роли с тестовым сообщением — `POST /api/check/telegram/{роль}`, решение 176), площадка, канал отправки |
| `usdt_transfer.rs` | Решение 176: «Перевести USDT» в интерфейсе — `GET /api/usdt-transfer/sources` (площадки с USDT), `POST /api/usdt-transfer/quote` (сводка); отправка — `POST /api/manual/command` `{"TransferUsdt": …}` |
| `secrets.rs` | Секреты |
| `ws.rs` | Живой поток |
| `static_ui.rs` | Раздача собранного интерфейса, 404 для неизвестных адресов API |
