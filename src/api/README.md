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
| `GET /api/venues/{id}/dex-families` | DEX и виды пулов адаптера (id области «DEX»); биржа — 400 «не применимо» |
| `GET /api/channels/{venue}` | Каналы отправки адаптера DEX (id области «канал»): `[{info, stats}]`; `stats: null` — ядро ещё не отдаёт статистику |
| `GET /api/journal?limit=N[&before=ID]` | Журнал |
| `GET /api/coins`, `POST /api/coins/recheck` | Экран «Монеты»: справочник (сейчас 503 «нет данных» — ядро не подключено); «Перепроверить» — шлюз `recheck_coins` |
| `GET /api/manual`, `POST /api/manual/command` | Экран «Ручное управление» (сейчас 503: нет данных, команда не выполняется и пишется в журнал) |
| `GET /api/gas-stats` | Экран «Статистика газа» (сейчас 503 «нет данных») |
| `POST /api/check/telegram` | «Проверить подключение» Telegram |
| `POST /api/check/venue/{id}` | «Проверить подключение» площадки |
| `POST /api/check/channel/{venue}/{id}` | «Проверить подключение» канала отправки (шлюз `check_send_channel`) |
| `GET /api/secrets/known` | Известные секреты: имя, площадка, описание |
| `GET /api/secrets/status` | Создан ли файл, разблокировано ли, имена секретов (без значений) |
| `POST /api/secrets/create`, `/unlock`, `/lock`, `/password` | Создать файл, разблокировать, заблокировать, сменить пароль |
| `PUT /api/secrets/{name}`, `DELETE /api/secrets/{name}` | Добавить/заменить, удалить секрет (нужен пароль) |
| `GET /api/ws` | Живой поток: `settings`, `journal`, `lagged` |
| `GET /api/{неизвестно}` | 404 `{"error": …}` |
| `/` и всё остальное | Собранный интерфейс из `--ui-dir` (нет файла — `index.html`); без `--ui-dir` — не раздаётся |

Ответ ошибки: `{"error": "понятная причина"}`. «Нет данных» (источник ещё не подключён) — 503 с причиной, а не пустой список. Ошибка шлюза (`ApiError::gateway`): `NotApplicable` — 400,
площадка не подключена (`VenueNotConnected`) — 404, остальные коды — 502; текст — «площадка (код): сообщение». Чужие Host/Origin отклоняются (`guard.rs`). Авторизации пока нет — запускать только на 127.0.0.1 (открытый вопрос).

## Маршруты экранов DEX и ядра (контракт)

Реализованы в `dex.rs`, `coins.rs`, `control.rs`, `check.rs`. Что берёт данные из ядра (`/api/coins`, `/api/manual`,
`/api/gas-stats`, `stats` каналов, WS `coin_check`/`pool_check`/`coin_block`/`stuck_alert`/`manual`/`send_channel`/`gas_stats`) —
ждёт подключения ядра к `AppState` (запрос на изменение стыка — Issue #9); до тех пор 503 «нет данных» / `stats: null`.

Типы — из `src/core/types/` и `src/gateway/contract.rs` (JSON — как их serde). Ошибка — `{"error": …}`.
Данные приходят из ядра (оно хранит последние события адаптеров) и из реестра шлюза; к адаптерам напрямую api не ходит.
Настройки этих экранов меняются обычным `PUT /api/settings` (области `coin`, `dex`, `channel`).

### Экран «Монеты» (налоги, крючки, чёрные списки)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/coins` | — | `[{asset: Asset, venues: [VenueId], taxes: CheckState<TokenTaxes>, tax_source, checked_at, block: CoinBlock \| null, pools: [{venue, family, kind: PoolKind, pool: PoolRef, fee_pips, hooks: HookInfo \| null, check: PoolCheck \| null, block, received_at}]}]` |
| `POST /api/coins/recheck` | `{venue: VenueId, tickers: [String]}` | `{ok, details}` — шлюз `recheck_coins`; биржа отвечает «не применимо» |
| ручной чёрный список | `PUT /api/settings` `{name:"coin.blocked", scope:{kind:"Coin",id:"CAKE"}, value:true}` (+ `coin.blocked_note`) | как у настроек |
| WS `/api/ws` | — | `{"type":"coin_check","data":CoinCheck}`, `{"type":"pool_check","data":PoolCheck}`, `{"type":"coin_block","data":{asset, block: CoinBlock \| null}}` (null — вышла из списка) |

### Экран «Ручное управление» (решение 37)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/manual` | — | `{alerts: [StuckAlert], holdings: [ManualHolding]}` |
| `POST /api/manual/command` | `{action: ManualAction}` — `{"SellNow":{"alert_id":…}}`, `{"TakeOver":{"alert_id":…}}`, `{"ReturnToBot":{"holding_id":…}}` | `{ok, details}`; источник команды — `CommandSource::Ui`, пишется в журнал |
| WS `/api/ws` | — | `{"type":"stuck_alert","data":StuckAlert}`, `{"type":"manual","data":{alerts, holdings}}` |

### Экран «Каналы отправки» (решения 33, 41–44)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/channels/{venue}` | — | `[{info: SendChannelInfo, stats: SendChannelStats \| null}]` — шлюз `send_channels` + последние события ядра; биржа — 400 «не применимо» |
| `POST /api/check/channel/{venue}/{id}` | — | `{ok, details}` — шлюз `check_send_channel`, пишется в журнал |
| настройки канала | `PUT /api/settings` с `scope:{kind:"Channel",id:"club48"}` (`channel.*`); ключ — `PUT /api/secrets/channel.<id>.key` | как у настроек и секретов |
| WS `/api/ws` | — | `{"type":"send_channel","data":SendChannelStats}` |

### Экран «Статистика газа» (решение 36)

| Маршрут | Запрос | Ответ |
| --- | --- | --- |
| `GET /api/gas-stats` | — | `[CoinGasStats]`, по монетам |
| WS `/api/ws` | — | `{"type":"gas_stats","data":CoinGasStats}` |

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
| `venues.rs` | Список площадок из реестра шлюза |
| `dex.rs` | DEX и виды пулов, каналы отправки — из реестра шлюза |
| `coins.rs` | Экран «Монеты»: справочник (нет данных до ядра), «Перепроверить» |
| `control.rs` | «Ручное управление» и «Статистика газа» (нет данных до ядра) |
| `journal.rs` | Журнал |
| `check.rs` | «Проверить подключение»: Telegram, площадка, канал отправки |
| `secrets.rs` | Секреты |
| `ws.rs` | Живой поток |
| `static_ui.rs` | Раздача собранного интерфейса, 404 для неизвестных адресов API |
