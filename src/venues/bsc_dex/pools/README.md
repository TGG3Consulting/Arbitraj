# venues/bsc_dex/pools — пулы DEX

Виды (id области «DEX»): `pancakeswap_v2`, `pancakeswap_v3`, `pancakeswap_infinity_cl`, `pancakeswap_infinity_bin`,
`uniswap_v2`, `uniswap_v3`, `uniswap_v4` (решения 13, 31). Адреса контрактов — только настройки `dex.*` по виду пула.

## Как работает

1. `market.rs`: USDT — монета с тикером `venue.quote_asset` [bsc_dex] из списка ядра (адрес — из API биржи).
2. `discover.rs`: V2 — `getPair(монета, USDT)`; V3 — история `PoolCreated`, V4 — история `Initialize` с USDT в
   индексированном поле, от `dex.scan_from_block` кусками `dex.logs_block_batch`. Прочитанное хранится в памяти —
   дальше дочитываются только новые блоки. У V4 PoolId считается по ключу и сверяется с событием.
3. `listen.rs`: на пачку пулов (`dex.pools_per_sub`) — подписка `logs`, затем чтение состояния на блоке B,
   затем события с блоком > B. Реорг (`removed`) или расхождение — пул перечитывается. Обрыв — `Invalidated`
   по монетам пачки, пауза `venue.ws_reconnect_pause_ms`, заново.
4. `created.rs`: подписка на события создания с USDT → дочитывание истории → слушатели новых пулов.
5. Каждое изменение — событие `Pool` ядру (без ожидания: свежее состояние всегда в реестре).

## Файлы

| Файл | Что делает |
| --- | --- |
| `families.rs` | Виды пулов: id, DEX, математика, протокол, ссылки на страницы адресов |
| `abi_v2v3.rs`, `abi_v4.rs` | ABI фабрик, пулов, менеджеров V4, StateView, TickLens (+ сверка topic0 с исследованием) |
| `key.rs` | PoolKey V4 (Infinity, Uniswap), PoolId, разбор parameters (тест — реальный пул из документации PancakeSwap) |
| `hooks.rs` | Флаги крючка и группа A/B/C (решение 32) |
| `scan.rs` | eth_getLogs кусками |
| `discover.rs`, `discover_v2.rs`, `discover_v3.rs`, `discover_v4.rs` | Поиск пулов |
| `load.rs`, `load_v2v3.rs`, `load_cl.rs`, `load_bin.rs` | Состояние на блоке: резервы, slot0, ликвидность, тики, бины |
| `ticks.rs`, `bins.rs` | Карта тиков и резервы бинов (в прочитанных пределах) |
| `events.rs`, `events_v2.rs`, `events_v3.rs`, `events_cl.rs`, `events_bin.rs` | Применение событий |
| `listen.rs` | Слушатель пачки пулов |
| `listen_log.rs` | Один лог: применить, перечитать, отправить ядру |
| `created.rs` | Новые пулы в реальном времени |
| `market.rs` | Подписка на рынок целиком |
| `registry.rs` | Реестр пулов в памяти |

## Расхождения в официальных источниках

- `getNextNonEmptyBin`: комментарий в `IBinPoolManager.sol` — «higher if swapForY», код (`BinPool.getNextNonEmptyBin`
  → `TreeMath.findFirstRight`) — строго ниже. Сделано по коду (`load_bin.rs`).
