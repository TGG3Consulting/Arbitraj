# api — веб-сервер для интерфейса (axum)

| Маршрут | Что делает |
| --- | --- |
| `GET /api/settings/schema` | Каталог настроек и событий уведомлений |
| `GET /api/settings` | Все заданные значения |
| `PUT /api/settings` | `{name, scope, value}`; `value: null` — очистить |
| `GET /api/settings/history?limit=N[&name=]` | История изменений |
| `GET /api/rules`, `POST /api/rules`, `PUT /api/rules/{id}` | Правила шлюза |
| `GET /api/rules/meta` | Справочник для формы правил: коды ошибок, действия с параметрами, виды области |
| `GET /api/venues` | Площадки, зарегистрированные в шлюзе (id по алфавиту) |
| `GET /api/venues/kinds` | Площадки с видом `[{id, kind: "Cex" \| "Dex"}]` — подпись направления перевода у настроек `venue_role: "TransferSource"` (решение 51) |
| `GET /api/venues/{id}/dex-families` | DEX и виды пулов адаптера (id области «DEX»); биржа — 400 «не применимо» |
| `GET /api/venues/{id}/pool-directory` | Справочник пулов от индексатора (решение 58): `PoolDirectory {source, network, attribution_text, attribution_url, dexes: [{id, name}], dexes_at, tokens: [{token, ticker, fetched_at, pages, total, error}], pools: [{token, dex_id, pool, base, quote, base_symbol, quote_symbol, reserve_usd, status}], error}`; `status` — `"Unchecked"`, `"Taken"`, `{"NotConnected": причина}`, `{"Rejected": причина}`, `{"Unsupported": причина}` (например нативный BNB); биржа — 400 |
| `GET /api/channels/{venue}` | Каналы отправки адаптера DEX (id области «канал»): `[{info, stats}]`; `stats: null` — по каналу ещё не было событий |
| `GET /api/wallet-queue/{venue}` | Очередь транзакций кошелька DEX: свободна / ждёт итога (nonce, хеш) / остановлена до ручной проверки (причина) |
| `POST /api/wallet-queue/{venue}/release` | «Снять очередь после ручной проверки» `{pending_ok}` — только остановленную (иначе 400); nonce ещё не занят без `pending_ok` — 409, интерфейс просит второе подтверждение |
| `GET /api/journal?limit=N[&before=ID]` | Журнал |
| `GET /api/coins`, `POST /api/coins/recheck` | Экран «Монеты»: справочник из ядра; «Перепроверить» — шлюз `recheck_coins` |
| `GET /api/manual`, `POST /api/manual/command` | Экран «Ручное управление»: алерты и монеты человека, команды ядру |
| `GET /api/gas-stats` | Экран «Статистика газа» |
| `GET /api/spreads`, `GET /api/candidates` | «Живой экран»: спреды направлений и последние расчёты кандидатов с причиной отказа |
| `GET /api/deals` | Экран «Сделки»: план и факт по шагам |
| `GET /api/transits` | История времени в пути (решение 50) |
| `GET /api/control`, `POST /api/control`, `POST /api/control/venue/{id}/resume`, `POST /api/control/quarantine/{ticker}/release` | Управление: режим «Старт/Стоп/Пауза», снятие паузы площадки и карантина монеты |
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
Область «DEX»: чей id у настройки — поле схемы `dex_id` (`"Dex"` — имя DEX, `"Family"` — вид пула, `"None"`).
Способ отправки канала ↔ вариант `channel.method`: `SendMethod::CHOICES` / `setting_value()` (core/types/send.rs).

### Экран «Монеты» (налоги, крючки, чёрные списки)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/coins` | — | `[{asset: Asset, venues: [VenueId], taxes: CheckState<TokenTaxes>, tax_source, checked_at, block: CoinBlock \| null, pools: [{venue, family, kind: PoolKind, pool: PoolRef, fee_pips, hooks: HookInfo \| null, check: PoolCheck \| null, pair: String, admission: String \| null, block: u64, received_at}], via: [{venue, via}], checks: [CoinCheck], problem: String \| null, presence: [Presence], directions: [Direction]}]` — `pools[].block` — номер блока, по которому получено состояние пула; `admission` — причина исключения пула (крючок, проверка), null — пул допущен; `pair` — второй токен пула (USDT или промежуточная монета); `via` — через какую промежуточную монету допущена монета без пула с USDT (решение 52); `pools[].depth: PoolDepth | null` — что о пуле знает адаптер DEX (решение 73, шлюз `pool_depths`): `buy_usd`, `sell_usd` — глубина покупки и продажи в полосе `band_pct` (USDT), `min_usd` — порог, `deep_buy`, `deep_sell` — вердикт с гистерезисом, `full` — полоса прочитана целиком, `depth_error` — почему не посчитана, `asleep` — слушается только добавление ликвидности, `valid`, `quoter` (`not_checked` / `ok` / `failed`) и `quoter_reason`, `tick_spacing`; `pools[].depth_unavailable` — почему адаптер не ответил |
| `POST /api/coins/recheck` | `{venue: VenueId, assets: [Asset]}` (тикер + адрес контракта: одинаковый тикер бывает разными токенами) | `{ok, details}` — шлюз `recheck_coins`; биржа отвечает «не применимо» |
| ручной чёрный список | `PUT /api/settings` `{name:"coin.blocked", scope:{kind:"Coin",id:"CAKE"}, value:true}` (+ `coin.blocked_note`) | как у настроек |
| WS `/api/ws` | — | `{"type":"coin_check","data":CoinCheck}`, `{"type":"pool_check","data":PoolCheck}`, `{"type":"coin_block","data":{asset, block: CoinBlock \| null}}` (null — вышла из списка) |

### Экран «Ручное управление» (решение 37)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/manual` | — | `{alerts: [StuckAlert], holdings: [ManualHolding]}` |
| `POST /api/manual/command` | `{action: ManualAction}` — `{"SellNow":{"alert_id":…}}`, `{"TakeOver":{"alert_id":…}}`, `{"ReturnToBot":{"holding_id":…}}` | `{ok, details}`; источник — `CommandSource::Ui`, пишется в журнал; id алерта — UUID; по закрытому алерту — ok и «проигнорирована» |
| WS `/api/ws` | — | `{"type":"stuck_alert","data":StuckAlert}`, `{"type":"manual","data":{alerts, holdings}}` |

### Экран «Каналы отправки» (решения 33, 41–44)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/channels/{venue}` | — | `[{info: SendChannelInfo, stats: SendChannelStats \| null}]` — шлюз `send_channels` + последние события ядра; биржа — 400 «не применимо» |
| `GET /api/wallet-queue/{venue}` | — | `WalletQueue {venue, state: Free \| Busy{nonce, hash, since} \| Halted{reason, nonce, hash, since}}` — шлюз `wallet_queue`; биржа — 400 |
| `POST /api/wallet-queue/{venue}/release` | `{pending_ok: bool}` | `WalletQueue` после снятия — шлюз `release_wallet_queue`; очередь не остановлена — 400; nonce не занят без `pending_ok` — 409 |
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
| `GET /api/candidates` | — | `[{direction, verdict, at}]`, новые сверху; `verdict` — `{"Accept": Calc}` или `{"Reject": {reason, best: Calc \| null}}`; `Calc.costs[]` — `{kind, stage, amount, asset, usdt \| null, in_price, in_qty, step: {index, from, to, from_token, to_token} \| null}` |
| `GET /api/deals` | — | `[Deal]` (core/deal.rs): шаг, где монеты, вложено, выручка, затраты, план, факт покупки/переводов/продаж, хронология, итог |
| `GET /api/transits` | — | `[{asset, from, to, count, avg_s}]` |
| `GET /api/control` | — | `Risk`: `{mode: "Stopped" \| "Running" \| "Paused", reason, venue_paused, errors_in_row, quarantine, day: {date, result_usd, final_usd, provisional_usd, provisional: {сделка: [сумма, день учёта]}}}` — итог дня с предварительной частью (решение 53) |
| `POST /api/control` | `{mode}` | `Risk`; пишется в журнал, уведомления `system.started` / `system.stopped` |
| `POST /api/control/venue/{id}/resume` | — | `Risk` — пауза площадки снята |
| `POST /api/control/quarantine/{ticker}/release` | — | `Risk` — карантин монеты снят, `route.quarantine_out` |
| WS `/api/ws` | — | `spread` (SpreadView), `candidate`, `deal` (Deal), `balances` (`{venues, in_transit_usd, per_coin_usd}`), `control` (Risk), `coins` (`{coins, directions}` — перечитать GET /api/coins) |

### DEX и виды пулов (экран «Площадки», настройки области «DEX»)

«Проверить подключение» узла BSC и кошелька — существующий `POST /api/check/venue/bsc_dex` (адаптер DEX, `check_connection`).

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/venues/{id}/dex-families` | — | `[DexFamily]` — шлюз `dex_families`; биржа — 400 «не применимо» |

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
| `venues.rs` | Список площадок из реестра шлюза |
| `dex.rs` | DEX и виды пулов, каналы отправки — из реестра шлюза |
| `coins.rs` | Экран «Монеты»: справочник из ядра, «Перепроверить» по Asset |
| `control.rs` | «Ручное управление» и «Статистика газа» — данные и команды ядра |
| `core_api.rs` | «Живой экран», «Сделки», время в пути, «Управление» (Старт/Стоп/Пауза) |
| `journal.rs` | Журнал |
| `check.rs` | «Проверить подключение»: Telegram, площадка, канал отправки |
| `secrets.rs` | Секреты |
| `ws.rs` | Живой поток |
| `static_ui.rs` | Раздача собранного интерфейса, 404 для неизвестных адресов API |
