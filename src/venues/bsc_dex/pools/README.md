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
   Ликвидность индексатора не участвует ни в чём. Пул, который Universal Router не исполняет сам (check/emitter.rs), исключён.
   Второй источник (решение 121, C3) — поиск по фабрикам на своём узле (`factory_scan.rs`, включается `index.factory_scan`):
   V2 getPair, V3 getPool по `dex.v3_fee_tiers` DEX; пачки Multicall3 `dex.load_batch`; найденное — та же проверка и подписка.
   Решение 292: тем же проходом — пулы из `dex.stable_pools` видов saddle_stableswap / curve_stableswap (`stable_list.rs`):
   монеты читаются у самого пула, кандидат — каждая пара (монета, встречный), обе монеты которой в пуле.
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
| `factory_scan.rs` | Поиск пулов по фабрикам на своём узле (решение 121, C3; index.factory_scan): проход при старте подписки и при изменении настроек, найденное — в справочник (factory) и в ту же проверку и подписку, что пулы индексатора (канал «монета обновлена») |
| `factory_ask.rs` | Вид вопроса к реестру фабрики при поиске пулов (решения 121 C3, 122 F): V2 getPair, V3 getPool(fee), Solidly getPool / Solidly V1 getPair (по stable), Algebra poolByPair, DODO V2 getDODOPoolBidirection; данные вызова и разбор ответа в список адресов |
| `factory_calls.rs` | Вызовы реестра фабрик: V2 getPair(монета, встречный), V3 getPool(…, fee) по dex.v3_fee_tiers; заметки по DEX (не подключена, не задана dex.v3_fee_tiers); разбор ответа |
| `families.rs` | DEX из таблицы сопоставления (dex.kind): вид, протокол; подключена ли DEX (classify), исполняет ли её умный роутер |
| `exec_kind.rs` | Вид шага контракта-исполнителя у вида пула (решение 122 F): V2 / V3 / ALGEBRA / SOLIDLY / DODO / CURVE / MAVERICK / STABLE_U256 (PancakeSwap Stable, Lista) / SADDLE (Nerve) / NOMI_STABLE (решение 284, только v2) / METRIC (решение 285, только v2); `via_executor` — вид шага есть И задан exec.executor_address (без адреса — сигнал); ревью 281 — и обратный вызов dex.v3_swap_callback есть у исполнителя (exec_callback.rs) |
| `graph.rs` | Граф токенов в ОЗУ по всем пулам (решение 80): снимок реестра, рёбра — пулы, фильтр по достоверности/глубине (решение 73), не больше dex.route_max_pools_per_edge на пару; индекс «токен → рёбра» для перебора путей |
| `graph_cache.rs` | Кэш графа маршрутов: один граф на состояние реестра и блок, одно построение за раз (решения 129, 131); решение 217 — закреплённый набор путей у замороженного реестра (`pinned`) |
| `abi_check.rs` | ABI проверки пула: поля пула V2/V3, ключ у менеджера Infinity и у PositionManager Uniswap V4 |
| `index.rs` | Справочник → пулы подписки: сопоставление, проверка, итог по пулу (взят / отклонён / не проверен) |
| `load_batch.rs` | Чтение состояния многих пулов на одном блоке ступенями через Multicall3 (решение 61): головы (V2 резервы, V3/CL slot0 + liquidity) → слова тиков → данные тиков CL; частями по dex.load_batch; Bin — по одному |
| `load_batch_v2v3.rs` | Вызовы и разбор ступеней V2/V3 (TickLens) и выполнение ступени: все вызовы всех пулов вместе, ответы — по пулам; у DEX с dex.v3_fee_source = factory_swap_fee к голове V3 — swapFee фабрики (решение 274) |
| `fee_override.rs` | Решение 274 (SheepDex): комиссию задаёт фабрика по адресу получателя — swapFee(кошелёк/исполнитель, 0) читается вместе с пулом; не 0 — пул не прочитан с причиной |
| `fee_probe.rs` | Решение 274 (защита): при проверке пулов V3 — проба `swapFee(0x0, 0)` фабрики (один вызов на DEX, ответ в памяти до перезапуска); фабрика отвечает, а dex.v3_fee_source не задана — пулы DEX не берутся с причиной. Ревью 281: проба пропускается только при factory_swap_fee; менеджер комиссии (dex.v3_registry_fee_method / dex.v3_fee_manager) без pool_fee_each_read — пулы не берутся |
| `registry_key.rs` | Решение 281 (SquadSwap V3): ключ реестра фабрики — метод пула dex.v3_registry_fee_method (defaultFee()) читается при проверке, getPool и QuoterV2 — по нему; ключ в памяти по адресу пула |
| `fee_fresh.rs` | Решение 281: dex.v3_fee_source = pool_fee_each_read — fee() пула с каждой головой V3 (пачка и по одному), свежая комиссия — в запись (Loaded.fee_pips) |
| `fee_watch.rs` | Решение 281: подписка на события менеджера комиссии (dex.v3_fee_manager: SetCustomFee / ToggleCustomFee) и UpdatePoolManager фабрики — fee() пула перечитывается в реестр |
| `fee_refresh.rs` | Решение 281 (ревью): перечтение fee() одного пула (событие менеджера) и всех пулов DEX пачкой (начало каждой сессии fee_watch, смена менеджера) в реестр |
| `exec_callback.rs` | Ревью 281, В4: dex.v3_swap_callback — селектор обратного вызова пула в коде исполнителя ТОЙ версии, что сверена у кошелька (задача t114: v2 — contracts/out/ArbExecutor.bin-runtime, v1 — contracts/out/v1/; не сверен — нет пути с причиной); нет — пулы DEX без пути исполнения; решение 284 — вид шага только v2 (NOMI_STABLE) у кошелька с исполнителем v1 — тоже без пути (`step_missing`) |
| `abi_squad.rs` | Решение 281: ABI событий комиссии SquadSwap V3, фильтры подписки, пул из события |
| `load_batch_cl.rs` | Вызовы и разбор ступеней 1–2 CL (Infinity CL у менеджера, Uniswap V4 через StateView) |
| `load_batch_ticks.rs` | Ступень 3: данные установленных тиков CL всех ждущих пулов вместе |
| `reload_many.rs` | Чтение многих пулов группами, у каждой свежий блок; прочитанные — в реестр и ядру |
| `loading.rs` | Пулы, ждущие первого чтения: монеты — ядру событием PoolsLoading при изменении; сколько пулов монеты ещё ждёт (pending) — проверка налогов монеты ждёт только своих пулов (решение 61) |
| `order.rs` | Порядок загрузки взятых пулов (решение 61): монеты подписки, внутри — по ликвидности индексатора по убыванию, между монетами по кругу. Только порядок: ни один пул не отбрасывается (решение 58) |
| `index_watch.rs` | Во время подписки: перепроверка по обновлённым монетам, по изменению dex.*, повтор после сбоя узла; подключение новых; своя комиссия пар V2 |
| `dex_ids.rs` | Решение 128: полный список id области «DEX» — строки с видом + DEX из справочника пулов + id с уже заданными настройками (merge, extra) |
| `dex_rules.rs` | Умный роутер (smart_ok), вид не изменился (still), список DEX для интерфейса |
| `verify.rs`, `verify_calls.rs`, `verify_rules.rs`, `verify_v4.rs` | Проверка пула на своём узле (V2/V3 — поля и реестр фабрики, своя комиссия V2; V4 — ключ) |
| `verify_dodo.rs` | Проверка пула DODO PMM (решение 86b): реестр фабрики DVM/DPP/DSP (getDODOPoolBidirection) содержит пул |
| `verify_curve.rs` | Проверка пула StableSwap (решение 122 F): пул в dex.stable_pools DEX и пара среди его coins(k) |
| `verify_curve_ng.rs` | Проверка пула Curve StableSwap-NG (решение 275): реестр фабрики dex.factory — get_coins(pool) не пуст, is_meta = false, пара среди монет |
| `verify_lb.rs` | Проверка пула Liquidity Book V2.1 (решение 290): getFactory = dex.factory, токены X/Y = монета и встречный, реестр фабрики getLBPairInformation(X, Y, binStep) = этот адрес (самозванец отсекается) |
| `verify_balancer.rs` | Проверка пула Balancer V2 (решение 289): getVault = dex.vault, getPool(getPoolId) хранилища = этот адрес (самозванец отсекается), isPoolFromFactory у dex.factory / dex.balancer_stable_factory, пара — среди getPoolTokens |
| `verify_metric.rs` | Проверка пула Metric OMM (решение 285): isPool фабрики dex.factory + getImmutables (фабрика, токены пары; порядок токенов пула любой) |
| `verify_izi.rs` | Проверка пула iZiSwap / Biswap V3 (решение 286): поля пула factory = dex.factory, tokenX / tokenY = пара, fee; реестр фабрики pool(x, y, fee) = пул (uint24 у iZiSwap, uint16 у Biswap V3) |
| `verify_stable_u.rs` | Проверка пулов PancakeSwap Stable (реестр фабрики V2 getPairInfo/getThreePoolPairInfo или dex.stable_pools), Lista Stable (getPairInfos), Saddle/Nerve (dex.stable_pools) и пар WooFi (общий контракт dex.pmm_pool: quoteToken, wooracle = dex.oracle, реализация = dex.pool_implementation, цена оракула; ссылка PoolRef::Member) — решение 122 F, 3 |
| `verify_inf_stable.rs` | Проверка пула PancakeSwap Infinity Stable (решение «122 (Infinity Stable)»): PoolId → poolIdToPoolKey менеджера, крючок в dex.stable_hooks, флаги beforeSwap + ReturnsDelta, валюты; сборка Found из ключа (ссылка — адрес крючка, ключ — для обмена) |
| `verify_woo.rs` | Решение по паре WooFi (quoteToken, wooracle = dex.oracle, реализация прокси = dex.pool_implementation, цена оракула) и сборка найденного пула |
| `verify_maverick.rs` | Проверка пула Maverick V2 (решение 122 F): isFactoryPool фабрики, tokenA/tokenB = пара, permissionedSwap = false |
| `verify_solidly.rs` | Проверка пула Solidly (Topaz v2, THENA V1; решения 80, 85): реестр фабрики getPool(coin,quote,stable) для обоих stable содержит пул |
| `created_log.rs` | Событие создания → проверенный пул → подписка (V2/V3 PairCreated/PoolCreated, V4 Initialize, Elastic PoolCreated, DODO NewDVM/NewDPP/NewDSP, Solidly PoolCreated, Slipstream PoolCreated по int24 tickSpacing) |
| `abi_v2v3.rs`, `abi_v4.rs` | ABI фабрик, пулов, менеджеров V4, StateView, TickLens (+ сверка topic0 с исследованием) |
| `abi_elastic.rs` | ABI пула/фабрики/читателя тиков KyberSwap Elastic (решение 81): getPoolState/getLiquidityState/swapFeeUnits/tickDistance/ticks, Swap/Mint/Burn/BurnRTokens, getPool, getTicksInRange |
| `abi_dodo.rs` | ABI пула/фабрики DODO PMM (решение 86b): getPMMStateForCall/getUserFeeRate/_BASE_TOKEN_/_QUOTE_TOKEN_/querySellBase/querySellQuote, DODOSwap/BuyShares/SellShares/RChange/DODOFlashLoan, getDODOPoolBidirection, NewDVM/NewDPP/NewDSP |
| `abi_curve.rs` | ABI пула StableSwap шаблона Curve 2020 — Ellipsis (решение 122 F): coins/balances/A/fee/get_dy/exchange, события; topic0 событий с массивами uint256[N] — по числу монет (topics_for), MAX_COINS = 8 |
| `abi_curve_ng.rs` | ABI пула Curve StableSwap-NG v7.0.0 и фабрики NG (curvefi/stableswap-ng 3332cd4): геттеры, get_dy, события с uint256[], get_coins/is_meta |
| `abi_nomi.rs` | ABI пары Nomiswap Stable (решение 284; Sourcify full match 56/0x1e40…60f1): getReserves, swapFee, getA, множители, getAmountOut(address, uint256) — 0xca706bcf; события Sync/Swap/Mint/Burn (подписи V2), RampA/StopRampA; константы MAX_FEE, A_PRECISION, MAX_LOOP_LIMIT |
| `abi_dmm.rs` | ABI KyberSwap Classic DMM (решение 287; dmm-smart-contracts@e557b57, фабрика BSC — Sourcify match): пул getTradeInfo и события (Sync с виртуальными резервами, Swap, Mint, Burn, UpdateEMA), фабрика getPools / isPool / PoolCreated, роутер DMMRouter02.getAmountsOut (квотер сверки) |
| `abi_smardex.rs` | ABI пары SmarDex (решение 288; фабрика BSC — Sourcify match = SmarDex-Ecosystem/dex-smart-contracts deployments/bsc): getReserves, getFictiveReserves, getPriceAverage, getPairFees, события пары; Multicall3.getCurrentBlockTimestamp; реестр и поля пары — как V2 |
| `abi_lb.rs` | ABI Liquidity Book V2.1 (решение 290; lfj-gg/joe-v2@v2.1.0 = Sourcify): пул LBPair — getFactory, getTokenX/Y, getBinStep, getActiveId, getBin, getNextNonEmptyBin, параметры комиссии, getSwapOut, события; фабрика — getLBPairInformation |
| `abi_balancer.rs` | ABI Balancer V2 (решение 289; ACSI Finance BSC, исходник — balancer-deployments 20210418-vault / 20210418-weighted-pool / 20210624-stable-pool = Sourcify): Vault — getPool, getPoolTokens, queryBatchSwap; пул — getVault, getPoolId, getSwapFeePercentage, getPausedState, getNormalizedWeights, getAmplificationParameter, события (комиссия, пауза, рампа A); фабрика — isPoolFromFactory |
| `abi_metric.rs` | ABI пула Metric OMM и фабрики (решение 285; SDK @nradko/metric-omm-sdk-v1@0.4.5 + селекторы в коде пулов BSC): getImmutables (24 поля), getSellAndBuyPrices, swap, isPool; topic0 событий — из записанных логов |
| `abi_izi.rs` | ABI пулов iZiSwap и Biswap V3 (решение 286): геттеры пула, реестр фабрики (две подписи), state(), topic0 событий обоих видов (исходник iZiSwap-core@3fe21eba, Sourcify BiswapPoolV3 / BiswapFactoryV3, факт цепи tests/data/izi_2026-11-15) |
| `abi_stable_u.rs` | ABI PancakeSwap / Lista StableSwap (индексы uint256, is_killed / paused, A с точностью и проверка цены Lista, реестры фабрик, события) — решение 122 F, 3 |
| `abi_inf_stable.rs` | ABI крючка CLStableSwapHook (verified-код BscScan 0x86c3DC08…): геттеры состояния, get_dy, события TokenExchange / ликвидность / RampA / ApplyNewFee |
| `abi_saddle.rs` | ABI пула Saddle — Nerve (getToken/getTokenBalance uint8, getAPrecise, swapStorage, paused, calculateSwap, swap с deadline, события) |
| `abi_woo.rs` | ABI WooFi WooPPV2 и оракула по публичным интерфейсам (quoteToken, wooracle, paused, query/tryQuery, WooSwap, Upgraded; state(base)) и слот реализации EIP-1967 |
| `abi_maverick.rs` | ABI Maverick V2 (решение 122 F): пул (getState, fee, tickSpacing, tokenA/B, масштабы, события), фабрика (isFactoryPool, lookup, PoolCreated), PoolLens (getTicksAroundActive), Quoter (calculateSwap) — v2-common d91e871, v2-interfaces f9f4f3e |
| `abi_solidly.rs` | ABI пула/фабрики Solidly (Topaz v2, THENA V1; решения 80, 85): metadata/getReserves/stable/getAmountOut, Sync(uint256,uint256), getPool/isPool/getFee, PoolCreated (velodrome-finance HEAD b3065d8) |
| `abi_slipstream.rs` | ABI пула/фабрики Slipstream (Topaz v3; решения 80, 85): slot0 форка (6 полей, без feeProtocol)/liquidity/fee/tickSpacing/tickBitmap/ticks (10 полей), CLFactory getPool(a,b,int24)/PoolCreated(…,int24 tickSpacing,pool) (velodrome-finance/slipstream HEAD 193a6b8); Swap/Mint/Burn берутся из abi_v2v3 (идентичны Uni V3) |
| `key.rs` | PoolKey V4 (Infinity, Uniswap), PoolId, разбор parameters (тест — реальный пул из документации PancakeSwap) |
| `hooks.rs` | Флаги крючка и группа A/B/C (решение 32) |
| `scan.rs` | eth_getLogs кусками, ход чтения в журнал без спама |
| `history_v4.rs` | Фоновая загрузка истории V4: старт из памяти / SQLite / блока развёртывания, куски, подключение пулов по мере нахождения |
| `history_start.rs` | Стартовая точка истории V4: память → SQLite без узла; блок развёртывания — только при dex.history_scan = да (решение 121, C1) |
| `history_store.rs` | История V4 в SQLite: события найденных пулов и последний прочитанный блок по менеджеру |
| `attach.rs` | Найденные позже пулы — в открытую пачку своего вида (её соединение), заполнена — новая пачка |
| `group.rs` | Подписки пачки на одном соединении: «все события» и «только ликвидность», смена состава без переподключения |
| `admit.rs` | Приём пулов в пачку (подписка → блок → чтение → глубокие/мелкие) и возврат выросшего пула |
| `depth.rs` | Глубокий пул или мелкий (по обоим направлениям, пустой; при receipts не спят), пересчёт глубины на каждой отправке пула ядру (touch) — решение 73 |
| `depth_book.rs` | Глубина пулов по направлениям в памяти: пересчёт на смене состояния, гистерезис dex.depth_hysteresis_pct, сигнал «пул готов» при росте (решение 73); решение 217 — `verdicts` (копия вердиктов в момент снимка) и `seeded` |
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
| `load_curve.rs` | Состояние пула StableSwap на блоке (решение 122 F): coins(k)/balances(k) до первого отката, A(), fee() одним Multicall3 + rates по decimals монет → PoolState::StableSwap |
| `load_curve_ng.rs` | Состояние пула Curve NG на блоке одним Multicall3: N_COINS, coins/balances, stored_rates, A_precise, fee, offpeg → StableSwap с шаблоном NG |
| `load_nomi.rs` | Состояние пары Nomiswap Stable на блоке одним Multicall3: token0, getReserves, swapFee, getA, множители → StableSwap шаблона input_fee (решение 284) |
| `load_dmm.rs` | Состояние пула KyberSwap Classic DMM на блоке одним getTradeInfo (решение 287): реальные и виртуальные резервы, комиссия блока → PoolState::AmpV2; перечитывается каждым блоком (комиссия по номеру блока) |
| `load_smardex.rs` | Состояние пары SmarDex на блоке одной пачкой (решение 288): резервы, фиктивные резервы, средняя цена и её время, комиссии, время блока → PoolState::Fictive; перечитывается каждым блоком |
| `load_lb.rs` | Состояние пула Liquidity Book V2.1 на блоке (решение 290): getActiveId, параметры комиссии, getTokenX, getBinStep и время блока одной пачкой + полоса бинов; calls / decode — для пачки всех пулов каждым блоком; PoolState::LbBin |
| `load_lb_bins.rs` | Полоса бинов пула Liquidity Book V2.1 (решение 290): getNextNonEmptyBin / getBin пула в обе стороны до dex.bin_depth_usd (у пары без USDT — до конца); many — затронутые событием бины одной пачкой getBin (ревью 290) |
| `load_balancer.rs` | Состояние пула Balancer V2 на блоке одной пачкой (решение 289): getPoolTokens хранилища, комиссия, пауза пула и хранилища (ревью 289), веса / A; calls / decode — для пачки всех пулов; id, вид кривой (фабрика) и decimals — один раз на пул; PoolState::VaultPool |
| `load_balancer_fixed.rs` | Неизменяемое у пула Balancer V2 (решение 289): id, вид кривой (какая фабрика создала — isPoolFromFactory), множители точности по decimals; один раз на пул, в памяти |
| `per_block_lb.rs` | Решение 290: состояние всех пулов Liquidity Book V2.1 на новом блоке одной пачкой (load_lb::calls / decode + одно время блока), бины записи — как есть (меняют только события пула) |
| `per_block_balancer.rs` | Ревью 289: все прочитанные пулы Balancer V2 на новом блоке — одной пачкой (load_balancer::calls / decode, частями по dex.load_batch), в реестр и ядру как reload::load_one; пул без неизменяемого в памяти — обычным путём |
| `load_metric.rs` | Состояние пула Metric OMM на блоке (решение 285): getImmutables (token0 пула), getSellAndBuyPrices (пригодна ли цена провайдера) → OraclePmm с лесенкой |
| `load_metric_ladder.rs` | Лесенка ответов пула Metric через виртуальный квотер quoteMetric (решение 285; суммы dex.pmm_ladder_usd, только пары с USDT); общий проход read_with — и для iZiSwap (quoteIzi, решение 286) |
| `load_izi.rs` | Состояние пула iZiSwap / Biswap V3 на блоке (решение 286): tokenX, tokenY, state() (sqrtPrice_96) и лесенка ответов пула через quoteIzi; ядро OraclePmm, pool_token0 = tokenX |
| `load_stable_u.rs` | Состояние пула PancakeSwap / Lista StableSwap на блоке (у Lista — A по рампе на время блока, проверка цены, пауза) |
| `load_inf_stable.rs` | Состояние пула Infinity Stable у крючка одним Multicall3: N_COINS, coins, balances, stored_rates (внешние курсы), A_precise, fee, offpeg_fee_multiplier, pool_manager (сверка с dex.pool_manager); откат stored_rates — пул стоит |
| `stable_list.rs` | Решение 292: пулы StableSwap из dex.stable_pools (Saddle — getToken(k), Curve 2020 — coins(k)) — кандидаты справочника при поиске по фабрикам без индексатора; разбор пачки — sort (ответы пула вычитываются целиком); вызывает factory_scan.rs |
| `stable_native.rs` | Нативный BNB в пулах PancakeSwap Stable / Lista («122 X (нативный BNB в стейблсвапах)»): метка 0xEeee… ↔ обёртка dex.native_wrapped — монеты пула в пару, индекс метки (StableData.native), пара для реестра фабрики; без сети |
| `load_saddle.rs` | Состояние пула Saddle (Nerve) на блоке (A_PRECISION, 256 раундов, откат при несходимости, пауза); разбор без сети — calls / parse / build (ревью 292) |
| `load_woo.rs` | Состояние пары WooFi на блоке: state(base) оракула, paused, wooracle (= dex.oracle), реализация прокси. Своей математики нет |
| `load_woo_ladder.rs` | Лесенка цен пула WooFi на блоке: query квотера контракта в обе стороны по dex.pmm_ladder_usd (Multicall3, откат — край лесенки) |
| `load_maverick.rs` | Состояние пула Maverick V2 на блоке (решение 122 F): getState, tickSpacing, fee(true/false), tokenA, масштабы + PoolLens.getTicksAroundActive(dex.tick_radius) одной пачкой Multicall3 по dex.load_batch → PoolState::TickBin |
| `load_solidly.rs` | Состояние пула Solidly на блоке (решения 80, 85): metadata() пула + getFee(pool,stable) фабрики → PoolState::Solidly (резервы, stable, decimals, комиссия) — по одному пулу. Решение 273: комиссия по dex.solidly_fee_source (если задан) со своим правилом и ограничения обмена с фабрики (gate) |
| `solidly_fee_read.rs` | Решение 273: комиссия пула Solidly по источнику из настроек (factory_pool / factory_stable / pool / factory_by_type; метод — keccak сигнатуры), правило (numerator над делителем / divisor); fee_call / rule — для пачки; ограничения — переэкспорт из solidly_gate_read.rs |
| `solidly_refresh.rs` | Решение 273: перечитывание комиссии и ограничений пулов Solidly с источником из настроек перед каждым проходом сверки с квотером (без события пула; как своя комиссия пар V2). Ревью 273: ограничения фабрики — один раз на DEX за проход, комиссии пулов — пачкой (call::try_chunks частями dex.verify_batch) |
| `solidly_gate_read.rs` | Решение 273 (вынесено из solidly_fee_read.rs): ограничения обмена с фабрики — пауза, предел цены газа (больше u64 — «предела нет», Info один раз); has_gate — сбой хранилища наружу |
| `load_slipstream.rs` | Состояние пула Slipstream (Topaz v3; решения 80, 85): slot0 форка + liquidity + тики прямыми геттерами tickBitmap/ticks (не TickLens) → PoolState::V3 + карта тиков — по одному пулу |
| `ticks.rs`, `bins.rs` | Карта тиков и резервы бинов (в прочитанных пределах) |
| `events.rs`, `events_v2.rs`, `events_v3.rs`, `events_cl.rs`, `events_bin.rs` | Применение событий |
| `events_elastic.rs` | События KyberSwap Elastic (решение 81): Swap → цена/тик/baseL; Mint/Burn/BurnRTokens → перечитывание (reinvestL/карта тиков) |
| `events_dodo.rs` | События DODO PMM (решение 86b): любое событие кривой (DODOSwap/BuyShares/SellShares/RChange/DODOFlashLoan) → перечитывание пула (событие не несёт пост-состояние кривой) |
| `events_curve.rs` | События StableSwap (решение 122 F): любое событие пула (обмен, ликвидность, комиссия, A) → перечитывание |
| `events_curve_ng.rs` | События Curve NG (решение 275): любое событие пула → перечитать (ещё — каждый блок) |
| `events_nomi.rs` | События пары Nomiswap Stable (решение 284): любое событие пары → перечитать (ещё — каждый блок: swapFee без события, рампа A) |
| `events_dmm.rs` | События пула KyberSwap Classic DMM (решение 287): любое известное событие (Sync, Swap, UpdateEMA, Mint, Burn) → перечитать пул (getTradeInfo) |
| `events_smardex.rs` | События пары SmarDex (решение 288): любое известное событие (Sync, Swap, FeesChanged, Mint, Burn) → перечитать пару |
| `events_lb.rs` | События пула Liquidity Book V2.1 (решение 290, ревью 290): Swap (бин и новый активный), Deposited/WithdrawnFromBins, CompositionFees, FlashLoan → перечитать только затронутые бины; CollectedProtocolFees — без изменений; StaticFeeParametersSet, ForcedDecay → перечитать пул |
| `events_balancer.rs` | События пула Balancer V2 (решение 289): SwapFeePercentageChanged, PausedStateChanged, AmpUpdateStarted / Stopped → перечитать; балансы меняет хранилище — перечитывание каждым блоком |
| `events_metric.rs` | События пула Metric OMM (решение 285): topic0 из записанных логов → перечитать (ещё — каждый блок) |
| `events_izi.rs` | События пулов iZiSwap / Biswap V3 (решение 286): любое известное событие → перечитать пул (ещё — каждым блоком, per_block.rs) |
| `events_stable_u.rs` | События PancakeSwap / Lista Stable, Saddle, WooFi и крючка Infinity Stable: любое известное → перечитать |
| `per_block.rs` | Перечитывание Lista Stable и WooFi на каждом новом блоке (оракул без событий пула) и всех пар общего контракта по его логу |
| `events_maverick.rs` | События Maverick V2 (решение 122 F): PoolSwap / PoolAddLiquidity / PoolRemoveLiquidity / PoolMigrateBinsUpStack / PoolSetVariableFee → перечитывание пула |
| `events_solidly.rs` | События Solidly (решения 80, 85): Sync(uint256,uint256) → новые резервы в PoolState::Solidly (stable/decimals/комиссия сохраняются) |
| `listen.rs` | Слушатель пачки пулов |
| `listen_log.rs` | Один лог: применить, перечитать, отправить ядру |
| `created.rs` | Новые пулы в реальном времени |
| `market.rs` | Подписка на рынок целиком |
| `batches.rs` | Раскладка пулов по пачкам подписки: по виду (и менеджеру V4), не больше dex.pools_per_sub |
| `market_mids.rs` | Пулы «монета — промежуточная» и наблюдатели новых таких пулов (решение 52) |
| `wrapped.rs` | Решение 270: WETH() роутеров V2 — нативная обёртка только dex.native_wrapped, чужая (fstswap) не «BNB»; `natives` — кому тикер нативной (при dex.intermediate_dex_priced = да — только dex.native_wrapped, 270 дополнение 2); `verdict` — итог «Проверить подключение» |
| `mids.rs` | Промежуточные монеты: настройки, тикер (биржа по адресу или «BNB» для обёртки dex.native_wrapped; не задана — для WETH() роутера V2; чужой WETH() — без тикера, решение 270; решение 276 — при dex.intermediate_dex_priced = да тикер нативной только у dex.native_wrapped, монета без биржи и любой другой WETH() — с подписью с цепи, `mid_dex.rs`); решение 239 — `pinned`: копия списка на момент снимка котировки |
| `mid_dex.rs` | Решение 276: подпись промежуточной без биржи — symbol() с цепи, если он из A–Z, a–z, 0–9, «.», «_», «-», не длиннее dex.intermediate_label_max_len и не совпадает с занятым тикером (BNB, монеты подписки DEX, промежуточные, dex.native_ticker, venue.quote_asset, venue.price_assets бирж; `busy`); иначе — адрес; причина — в журнал (сбой symbol() — Warn); `pick` — чистая |
| `registry.rs` | Реестр пулов в памяти |
| `registry_index.rs` | Записи реестра за `Arc` и индексы «пара → пулы», «монета (адрес) → пулы» (решение 129: без прохода всего реестра; решение 211 — ключ адрес, не тикер) |
| `registry_frozen.rs` | Решение 217: снимок записей реестра (`Arc`, копия при записи) для котировки монеты на момент решения (пулы монеты, «промежуточная — USDT», «WBNB — USDT» для газа; при dex.router_mode = graph — пулы путей монеты из живого кэша графа, без полного реестра) и замороженный реестр из него (свои индексы, кэш графа с закреплёнными путями момента снимка, книга глубины без журнала с вердиктами, скопированными в момент снимка) |
| `progress.rs` | Сигналы подписки: стартовая загрузка V2/V3, «пул готов», «пустой пул снова не пуст», журнал без спама (once, reason_key), состояние связи WS |
| `session.rs` | Одна сессия пачки: подписка, блок B, ожидание HTTP, чтение пулов, повтор чтения неудачных на новых блоках (dex.pool_load_retries), события |
| `link.rs` | Связь слушателей с узлом WS: одна запись на площадку при потере и при восстановлении |
| `history_gate.rs` | Пауза чтения истории V4 из интерфейса (dex.history_scan) без опроса — по изменению настройки |
| `http_wait.rs` | Ожидание, пока узел HTTP дойдёт до блока узла WS (начало сессии слушателя) |

## Расхождения в официальных источниках

- `getNextNonEmptyBin`: комментарий в `IBinPoolManager.sol` — «higher if swapForY», код (`BinPool.getNextNonEmptyBin`
  → `TreeMath.findFirstRight`) — строго ниже. Сделано по коду (`load_bin.rs`).

Algebra Integral (решение 188): `abi_algebra_integral.rs` (геттеры и события комиссии Integral), `load_algebra_integral.rs` (тики связным списком, комиссия fee() плагина), `events_algebra_integral.rs` (SwapFee/Fee; Swap/Mint/Burn — `events_algebra.rs`). Математика, проверка и исполнение — общие с Algebra V1.9.
