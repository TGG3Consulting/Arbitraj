# venues/bsc_dex/pools — пулы DEX

DEX — строки таблицы сопоставления (решение 58): id области «DEX» — id DEX у индексатора, вид — настройка `dex.kind`
(форк V2, форк V3 с ABI Uniswap или PancakeSwap, PancakeSwap Infinity CL / Bin, Uniswap V4, не поддерживается).
Списка DEX в коде нет: виды — перечень модулей адаптера (`families.rs`), адреса — настройки `dex.*` по DEX.

## Как работает

1. `market.rs`: USDT — монета с тикером `venue.quote_asset` [bsc_dex] из списка ядра (адрес — из API биржи).
   Промежуточные монеты (решение 52, `mids.rs`): пулы «промежуточная — USDT» ищутся вместе с пулами монет, пулы
   «монета — промежуточная» — тем же поиском со встречным токеном-промежуточной (`market_mids.rs`); у записи реестра
   поле `usdt` — встречный токен пула. Изменение `dex.intermediate_*` пересобирает подписку (`resubscribe.rs`).
2. Пулы — из справочника индексатора (`directory/`, решение 58): `index.rs` берёт пулы монет с USDT и промежуточными,
   DEX — по таблице (`families::classify`: не подключена или форк без пути исполнения — причина на экране «Монеты»),
   затем проверка на своём узле частями `dex.verify_batch` (`verify.rs`, Multicall3): V2/V3 — factory(), token0(),
   token1() (V3 — fee(), tickSpacing(), lmPool() — ABI PancakeSwap или Uniswap; V2 со своей комиссией —
   `dex.v2_fee_method`), затем реестр фабрики getPair /
   getPool должен вернуть тот же адрес; V4 (`verify_v4.rs`) — ключ у менеджера (Infinity: poolIdToPoolKey) или у
   PositionManager (Uniswap V4: poolKeys), keccak ключа = PoolId, менеджер и валюты совпадают. Отказ — причина в
   справочнике и один раз в журнал; сбой узла — «не проверен», повтор (`index_watch.rs`). При старте подписки —
   сохранённый справочник; обновление монеты, изменение dex.* или повтор — проверка и подключение новых (`attach.rs`).
   Ликвидность индексатора не участвует ни в чём. Пул, который Universal Router не исполняет сам (check/emitter.rs), исключён. Перебора фабрик по уровням комиссии больше нет.
   История V4 (`history_v4.rs`) — необязательна (`dex.history_scan`): нужна только для пулов Uniswap V4 без позиций через
   PositionManager; из памяти → из SQLite (`history_store.rs`) → от блока развёртывания (`deploy_block.rs`), куски
   `dex.logs_block_batch`, найденное подключается раз в `dex.scan_progress_s`. У V4 PoolId сверяется с событием.
3. `listen.rs` / `session.rs`: пачка пулов (не больше `dex.pools_per_sub`) — одно соединение WebSocket (`group.rs`).
   Приём (`admit.rs`): подписка `logs`, блок B того же узла, чтение состояния на B, затем события с блоком > B.
   Пулы мельче `dex.min_pool_liquidity_usd` по обоим направлениям в полосе `dex.depth_band_pct` или пустые
   (`depth.rs`, решение 73) — во второй подписке пачки, только
   добавление ликвидности (Mint / ModifyLiquidity); такое событие будит пул: чтение, и если он вырос — полная
   подписка с ним. Реорг (`removed`) или расхождение — пул перечитывается. Обрыв — `Invalidated` по монетам пачки,
   пауза `venue.ws_reconnect_pause_ms`, заново (с делением на глубокие и мелкие по свежему чтению). При источнике
   `receipts` пулы не спят; глубина по направлениям пересчитывается на каждой смене состояния (`depth_book.rs`).
4. `created.rs` / `created_log.rs`: подписка на события создания со встречным токеном у фабрик и менеджеров подключённых
   DEX; событие с монетой подписки → пул из события → проверка (`verify.rs`; V4 — ключ из события) → подписка.
   Пулы, найденные позже (новые и история V4), `attach.rs` отдаёт в открытую пачку своего вида — её сессия
   принимает их на том же соединении; пачка заполнена — новая пачка. Подписки не пересоздаются на каждый пул.
5. Каждое изменение — событие `Pool` ядру (без ожидания: свежее состояние всегда в реестре).

## Файлы

| Файл | Что делает |
| --- | --- |
| `families.rs` | DEX из таблицы сопоставления (dex.kind): вид, протокол; подключена ли DEX (classify), исполняет ли её умный роутер |
| `graph.rs` | Граф токенов в ОЗУ по всем пулам (решение 80): снимок реестра, рёбра — пулы, фильтр по достоверности/глубине (решение 73), не больше dex.route_max_pools_per_edge на пару; индекс «токен → рёбра» для перебора путей |
| `abi_check.rs` | ABI проверки пула: поля пула V2/V3, ключ у менеджера Infinity и у PositionManager Uniswap V4 |
| `index.rs` | Справочник → пулы подписки: сопоставление, проверка, итог по пулу (взят / отклонён / не проверен) |
| `load_batch.rs` | Чтение состояния многих пулов на одном блоке ступенями через Multicall3 (решение 61): головы (V2 резервы, V3/CL slot0 + liquidity) → слова тиков → данные тиков CL; частями по dex.load_batch; Bin — по одному |
| `load_batch_v2v3.rs` | Вызовы и разбор ступеней V2/V3 (TickLens) и выполнение ступени: все вызовы всех пулов вместе, ответы — по пулам |
| `load_batch_cl.rs` | Вызовы и разбор ступеней 1–2 CL (Infinity CL у менеджера, Uniswap V4 через StateView) |
| `load_batch_ticks.rs` | Ступень 3: данные установленных тиков CL всех ждущих пулов вместе |
| `reload_many.rs` | Чтение многих пулов группами, у каждой свежий блок; прочитанные — в реестр и ядру |
| `loading.rs` | Пулы, ждущие первого чтения: монеты — ядру событием PoolsLoading при изменении; сколько пулов монеты ещё ждёт (pending) — проверка налогов монеты ждёт только своих пулов (решение 61) |
| `order.rs` | Порядок загрузки взятых пулов (решение 61): монеты подписки, внутри — по ликвидности индексатора по убыванию, между монетами по кругу. Только порядок: ни один пул не отбрасывается (решение 58) |
| `index_watch.rs` | Во время подписки: перепроверка по обновлённым монетам, по изменению dex.*, повтор после сбоя узла; подключение новых; своя комиссия пар V2 |
| `dex_rules.rs` | Умный роутер (smart_ok), вид не изменился (still), список DEX для интерфейса |
| `verify.rs`, `verify_calls.rs`, `verify_rules.rs`, `verify_v4.rs` | Проверка пула на своём узле (V2/V3 — поля и реестр фабрики, своя комиссия V2; V4 — ключ) |
| `verify_dodo.rs` | Проверка пула DODO PMM (решение 86b): реестр фабрики DVM/DPP/DSP (getDODOPoolBidirection) содержит пул |
| `verify_solidly.rs` | Проверка пула Solidly (Topaz v2, THENA V1; решения 80, 85): реестр фабрики getPool(coin,quote,stable) для обоих stable содержит пул |
| `created_log.rs` | Событие создания → проверенный пул → подписка (V2/V3 PairCreated/PoolCreated, V4 Initialize, Elastic PoolCreated, DODO NewDVM/NewDPP/NewDSP, Solidly PoolCreated, Slipstream PoolCreated по int24 tickSpacing) |
| `abi_v2v3.rs`, `abi_v4.rs` | ABI фабрик, пулов, менеджеров V4, StateView, TickLens (+ сверка topic0 с исследованием) |
| `abi_elastic.rs` | ABI пула/фабрики/читателя тиков KyberSwap Elastic (решение 81): getPoolState/getLiquidityState/swapFeeUnits/tickDistance/ticks, Swap/Mint/Burn/BurnRTokens, getPool, getTicksInRange |
| `abi_dodo.rs` | ABI пула/фабрики DODO PMM (решение 86b): getPMMStateForCall/getUserFeeRate/_BASE_TOKEN_/_QUOTE_TOKEN_/querySellBase/querySellQuote, DODOSwap/BuyShares/SellShares/RChange/DODOFlashLoan, getDODOPoolBidirection, NewDVM/NewDPP/NewDSP |
| `abi_solidly.rs` | ABI пула/фабрики Solidly (Topaz v2, THENA V1; решения 80, 85): metadata/getReserves/stable/getAmountOut, Sync(uint256,uint256), getPool/isPool/getFee, PoolCreated (velodrome-finance HEAD b3065d8) |
| `abi_slipstream.rs` | ABI пула/фабрики Slipstream (Topaz v3; решения 80, 85): slot0 форка (6 полей, без feeProtocol)/liquidity/fee/tickSpacing/tickBitmap/ticks (10 полей), CLFactory getPool(a,b,int24)/PoolCreated(…,int24 tickSpacing,pool) (velodrome-finance/slipstream HEAD 193a6b8); Swap/Mint/Burn берутся из abi_v2v3 (идентичны Uni V3) |
| `key.rs` | PoolKey V4 (Infinity, Uniswap), PoolId, разбор parameters (тест — реальный пул из документации PancakeSwap) |
| `hooks.rs` | Флаги крючка и группа A/B/C (решение 32) |
| `scan.rs` | eth_getLogs кусками, ход чтения в журнал без спама |
| `history_v4.rs` | Фоновая загрузка истории V4: старт из памяти / SQLite / блока развёртывания, куски, подключение пулов по мере нахождения |
| `history_store.rs` | История V4 в SQLite: события найденных пулов и последний прочитанный блок по менеджеру |
| `attach.rs` | Найденные позже пулы — в открытую пачку своего вида (её соединение), заполнена — новая пачка |
| `group.rs` | Подписки пачки на одном соединении: «все события» и «только ликвидность», смена состава без переподключения |
| `admit.rs` | Приём пулов в пачку (подписка → блок → чтение → глубокие/мелкие) и возврат выросшего пула |
| `depth.rs` | Глубокий пул или мелкий (по обоим направлениям, пустой; при receipts не спят), пересчёт глубины на каждой отправке пула ядру (touch) — решение 73 |
| `depth_book.rs` | Глубина пулов по направлениям в памяти: пересчёт на смене состояния, гистерезис dex.depth_hysteresis_pct, сигнал «пул готов» при росте (решение 73) |
| `depth_key.rs` | Ключ кэша глубины: состояние пула, настройки, курс промежуточной монеты (решение 73) |
| `depth_log.rs` | Журнал смен «глубокий ↔ мелкий»: не чаще dex.depth_log_min_s, смена внутри окна — в конце окна (решение 73) |
| `depth_sync.rs` | Смена порога, полосы или гистерезиса глубины — пересчёт вердикта пулов пачки сразу, рост — «пул готов», спящий — пробуждение (решение 75) ; пересинхрон пачки после пропуска блока лентой (resync, решение 77) |
| `depth_facts.rs` | Пулы для экрана «Монеты»: глубина, вердикт, спит ли, сверка с квотером, tickSpacing (Venue::pool_depths, решение 73) |
| `invalidated.rs` | Событие ядру «пулы монеты недостоверны» без молчаливой потери |
| `session.rs` | Сессия пачки: приём, события, пробуждение мелких, новые пулы, обрыв |
| `reload.rs` | Чтение пула на блоке с отправкой ядру и повтор непрочитанных на новых блоках |
| `revive.rs` | Возврат в чтение пулов, брошенных из-за незаданной настройки (GatewayError.setting), когда её задали; при отставании рассылки настроек — по всем заданным |
| `deploy_block.rs` | Блок развёртывания менеджера V4: бинарный поиск eth_getCode, хранение в SQLite; доступ узла к истории состояния |
| `discover.rs`, `discover_v4.rs` | Найденный пул, встречный токен, пулы подписки из справочника; разбор Initialize и пул по ключу V4 |
| `load.rs`, `load_v2v3.rs`, `load_cl.rs`, `load_bin.rs` | Состояние на блоке: резервы, slot0, ликвидность, тики, бины |
| `load_elastic.rs` | Состояние пула KyberSwap Elastic на блоке (решение 81): getPoolState/getLiquidityState/swapFeeUnits + все тики списка через читатель (getTicksInRange) и ticks() — по одному пулу |
| `load_dodo.rs` | Состояние пула DODO PMM на блоке (решение 86b): getPMMStateForCall + getUserFeeRate(0) + _BASE_TOKEN_/_QUOTE_TOKEN_ → PoolState::Pmm; net_after_fees, rstate — по одному пулу |
| `load_solidly.rs` | Состояние пула Solidly на блоке (решения 80, 85): metadata() пула + getFee(pool,stable) фабрики → PoolState::Solidly (резервы, stable, decimals, комиссия) — по одному пулу |
| `load_slipstream.rs` | Состояние пула Slipstream (Topaz v3; решения 80, 85): slot0 форка + liquidity + тики прямыми геттерами tickBitmap/ticks (не TickLens) → PoolState::V3 + карта тиков — по одному пулу |
| `ticks.rs`, `bins.rs` | Карта тиков и резервы бинов (в прочитанных пределах) |
| `events.rs`, `events_v2.rs`, `events_v3.rs`, `events_cl.rs`, `events_bin.rs` | Применение событий |
| `events_elastic.rs` | События KyberSwap Elastic (решение 81): Swap → цена/тик/baseL; Mint/Burn/BurnRTokens → перечитывание (reinvestL/карта тиков) |
| `events_dodo.rs` | События DODO PMM (решение 86b): любое событие кривой (DODOSwap/BuyShares/SellShares/RChange/DODOFlashLoan) → перечитывание пула (событие не несёт пост-состояние кривой) |
| `events_solidly.rs` | События Solidly (решения 80, 85): Sync(uint256,uint256) → новые резервы в PoolState::Solidly (stable/decimals/комиссия сохраняются) |
| `listen.rs` | Слушатель пачки пулов |
| `listen_log.rs` | Один лог: применить, перечитать, отправить ядру |
| `created.rs` | Новые пулы в реальном времени |
| `market.rs` | Подписка на рынок целиком |
| `batches.rs` | Раскладка пулов по пачкам подписки: по виду (и менеджеру V4), не больше dex.pools_per_sub |
| `market_mids.rs` | Пулы «монета — промежуточная» и наблюдатели новых таких пулов (решение 52) |
| `mids.rs` | Промежуточные монеты: настройки, тикер (биржа по адресу или «BNB» для WETH() роутера V2) |
| `registry.rs` | Реестр пулов в памяти |
| `progress.rs` | Сигналы подписки: стартовая загрузка V2/V3, «пул готов», «пустой пул снова не пуст», журнал без спама (once, reason_key), состояние связи WS |
| `session.rs` | Одна сессия пачки: подписка, блок B, ожидание HTTP, чтение пулов, повтор чтения неудачных на новых блоках (dex.pool_load_retries), события |
| `link.rs` | Связь слушателей с узлом WS: одна запись на площадку при потере и при восстановлении |
| `history_gate.rs` | Пауза чтения истории V4 из интерфейса (dex.history_scan) без опроса — по изменению настройки |
| `http_wait.rs` | Ожидание, пока узел HTTP дойдёт до блока узла WS (начало сессии слушателя) |

## Расхождения в официальных источниках

- `getNextNonEmptyBin`: комментарий в `IBinPoolManager.sol` — «higher if swapForY», код (`BinPool.getNextNonEmptyBin`
  → `TreeMath.findFirstRight`) — строго ниже. Сделано по коду (`load_bin.rs`).
