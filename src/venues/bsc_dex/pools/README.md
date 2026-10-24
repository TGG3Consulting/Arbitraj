# venues/bsc_dex/pools — пулы DEX

Виды (id области «DEX»): `pancakeswap_v2`, `pancakeswap_v3`, `pancakeswap_infinity_cl`, `pancakeswap_infinity_bin`,
`uniswap_v2`, `uniswap_v3`, `uniswap_v4` (решения 13, 31). Адреса контрактов — только настройки `dex.*` по виду пула.

## Как работает

1. `market.rs`: USDT — монета с тикером `venue.quote_asset` [bsc_dex] из списка ядра (адрес — из API биржи).
   Промежуточные монеты (решение 52, `mids.rs`): пулы «промежуточная — USDT» ищутся вместе с пулами монет, пулы
   «монета — промежуточная» — тем же поиском со встречным токеном-промежуточной (`market_mids.rs`); у записи реестра
   поле `usdt` — встречный токен пула. Изменение `dex.intermediate_*` пересобирает подписку (`resubscribe.rs`).
2. `discover.rs`: V2 — `getPair(монета, USDT)`; V3 — `getPool(монета, USDT, fee)` по уровням `dex.v3_fee_tiers`, каждый
   сверен с `feeAmountTickSpacing` фабрики (истории событий нет); V4 — история `Initialize` менеджера (все пулы одним
   проходом, встречный токен выбирается из прочитанного) от блока развёртывания менеджера (`deploy_block.rs`: бинарный
   поиск `eth_getCode`, блок хранится в SQLite) кусками `dex.logs_block_batch`, ход — в журнал раз в `dex.scan_progress_s`.
   Прочитанное хранится в памяти — дальше дочитываются только новые блоки. У V4 PoolId считается по ключу и сверяется с событием.
3. `listen.rs`: на пачку пулов (`dex.pools_per_sub`) — подписка `logs`, затем чтение состояния на блоке B,
   затем события с блоком > B. Реорг (`removed`) или расхождение — пул перечитывается. Обрыв — `Invalidated`
   по монетам пачки, пауза `venue.ws_reconnect_pause_ms`, заново.
4. `created.rs`: подписка на события создания с USDT; событие с монетой подписки → повторный поиск → слушатели новых пулов.
5. Каждое изменение — событие `Pool` ядру (без ожидания: свежее состояние всегда в реестре).

## Файлы

| Файл | Что делает |
| --- | --- |
| `families.rs` | Виды пулов: id, DEX, математика, протокол, ссылки на страницы адресов |
| `abi_v2v3.rs`, `abi_v4.rs` | ABI фабрик, пулов, менеджеров V4, StateView, TickLens (+ сверка topic0 с исследованием) |
| `key.rs` | PoolKey V4 (Infinity, Uniswap), PoolId, разбор parameters (тест — реальный пул из документации PancakeSwap) |
| `hooks.rs` | Флаги крючка и группа A/B/C (решение 32) |
| `scan.rs` | eth_getLogs кусками, ход чтения в журнал без спама |
| `deploy_block.rs` | Блок развёртывания менеджера V4: бинарный поиск eth_getCode, хранение в SQLite; доступ узла к истории состояния |
| `discover.rs`, `discover_v2.rs`, `discover_v3.rs`, `discover_v4.rs` | Поиск пулов |
| `load.rs`, `load_v2v3.rs`, `load_cl.rs`, `load_bin.rs` | Состояние на блоке: резервы, slot0, ликвидность, тики, бины |
| `ticks.rs`, `bins.rs` | Карта тиков и резервы бинов (в прочитанных пределах) |
| `events.rs`, `events_v2.rs`, `events_v3.rs`, `events_cl.rs`, `events_bin.rs` | Применение событий |
| `listen.rs` | Слушатель пачки пулов |
| `listen_log.rs` | Один лог: применить, перечитать, отправить ядру |
| `created.rs` | Новые пулы в реальном времени |
| `market.rs` | Подписка на рынок целиком |
| `market_mids.rs` | Пулы «монета — промежуточная» и наблюдатели новых таких пулов (решение 52) |
| `mids.rs` | Промежуточные монеты: настройки, тикер (биржа по адресу или «BNB» для WETH() роутера V2) |
| `registry.rs` | Реестр пулов в памяти |

## Расхождения в официальных источниках

- `getNextNonEmptyBin`: комментарий в `IBinPoolManager.sol` — «higher if swapForY», код (`BinPool.getNextNonEmptyBin`
  → `TreeMath.findFirstRight`) — строго ниже. Сделано по коду (`load_bin.rs`).
