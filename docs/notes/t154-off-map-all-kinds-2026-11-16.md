# t154 — «цена ушла за прочитанную часть пула → пул сразу перечитывается» по ВСЕМ видам пулов (решение 332)

Ветка `t154/off-map-all-kinds` от `rel/dex-all` 8fe83aa1. Требование Тиграна 16.11.2026: решение глобальное — для всех
видов пулов, а не для одного. Ниже — по КАЖДОМУ виду (Proto из `src/venues/bsc_dex/pools/families.rs`, вид ядра PoolKind)
только факты кода этой ветки (файл:строка).

## Как устроено до t154

- Перечитывание «по выходу за прочитанное» есть одно: `pools/depth.rs` `off_map` (стр. 106–107: V3, Cl; Algebra — только
  при `dex.algebra_off_map_reread`, решение 320, `pools/off_map_algebra.rs:46`). Вызывается после события `Applied::Updated`
  (`pools/listen_log.rs:105`) и при смене полосы глубины на лету (`pools/depth_sync.rs:111`). Перечитывание —
  `pools/refresh_event.rs` (`load::refresh` на блоке события, класс `dex.event_refresh_urgency`).
- Пулы из бинов (Infinity Bin, LB V2.1): `depth.rs:106-108` для `PoolState::Bin`/`LbBin` даёт false; событие бинов —
  ветка `Applied::Bins` (`listen_log.rs:122`) перечитывает только затронутые бины; бин вне полосы молча не записывается
  (`pools/bins.rs:35`); расчёт обмена у края полосы останавливается с неполным итогом (`math/bin_swap.rs:56`,
  `math/lb_swap.rs:55`); глубина — до края полосы (`quote/depth_band.rs:435`, `:444`). LB раз в блок обновляет только
  активный бин и комиссию, бины прежние (`pools/per_block_lb.rs:77`). Итог: после сильного движения цены пул «мелкий» до
  Refresh-события или переподключения. Это и есть дыра.

## Таблица по видам

«Часть» — что бот держит не целиком. «Сейчас» — что происходит, когда цена/обмен выходит за прочитанное.

| Вид (dex.kind → Proto → PoolKind) | Что в памяти | Чем задан размер части | Выход за часть: было | Стало (t154) |
| --- | --- | --- | --- | --- |
| pancakeswap_v3, v3_fork_pancake → PancakeV3 → V3 | окно слов битмапа тиков | dex.tick_words, dex.tick_read_band_pct (решение 296) | перечитывание: depth.rs:106 → listen_log.rs:105 | действует, без изменений |
| uniswap_v3, v3_fork → UniswapV3 → V3 | то же | то же | то же | действует, без изменений |
| slipstream → Slipstream → V3 (PoolState::V3) | то же (load_slipstream.rs) | то же | то же (depth.rs:106) | действует, без изменений |
| pancakeswap_infinity_cl → InfinityCl → Cl | окно слов битмапа | то же | то же (PoolState::Cl, depth.rs:106) | действует, без изменений |
| uniswap_v4 → UniswapV4 → Cl | то же | то же | то же | действует, без изменений |
| thena_fusion → Algebra → Cl (PoolState::Algebra) | окно слов СЫРОГО битмапа | dex.tick_words, dex.tick_read_band_algebra_pct / dex.tick_read_band_pct | перечитывание при dex.algebra_off_map_reread = да (depth.rs:107, off_map_algebra.rs:46, решение 320); иначе — только Mint/Burn | действует под настройкой 320, без изменений |
| algebra_integral → AlgebraIntegral → Cl (PoolState::Algebra) | отрезок списка тиков | dex.integral_tick_steps | перечитывание при dex.algebra_off_map_reread = да, когда обмен в сторону невозможен из-за края окна (off_map_algebra.rs:65, решения 320/323) | действует под настройкой 320, без изменений |
| pancakeswap_infinity_bin → InfinityBin → Bin | полоса бинов вокруг активного | dex.bin_depth_usd в каждую сторону (load_bin.rs:92) | молча мелкий: бины вне полосы не пишутся (bins.rs:35), расчёт — неполный (bin_swap.rs:56) | **сделано**: off_map_bins.rs после бинов события (listen_log.rs ветка Bins), dex.bin_reread_left_pct [DEX] |
| lb_v21 → LbV21 → LbBin, пара с USDT | полоса бинов | dex.bin_depth_usd (load_lb_bins.rs:81) | молча мелкий (lb_swap.rs:55); per_block_lb.rs:77 бины не трогает | **сделано**: то же |
| lb_v21, пара без USDT | все бины пула (load_lb_bins.rs: depth None — сторона до конца) | — | неприменимо: полоса до конца пула (края 0 / 0xFFFFFF) | неприменимо (off_map_bins.rs check — false) |
| kyber_elastic → KyberElastic → Cl (PoolState::Elastic) | ВСЕ инициализированные тики (getTicksInRange от MIN_TICK, load_elastic.rs:58) | — | неприменимо: части нет | неприменимо |
| maverick_v2 → Maverick → TickBin | тики активный ± dex.tick_radius (load_maverick.rs) | dex.tick_radius | неприменимо: ЛЮБОЕ событие пула → Refresh, окно заново вокруг нового активного (events_maverick.rs:39-41) | неприменимо (окно центрируется на каждом событии; крупный обмен шире окна — частичный итог, это размер окна, а не сдвиг цены) |
| metric_omm → MetricOmm → OraclePmm | лесенка ответов квотера пула на суммах | dex.pmm_ladder_usd | неприменимо: лесенка строится заново у текущей цены на КАЖДОМ блоке (per_block.rs:56) | неприменимо (сумма больше последней ступени — частичный итог, это размер лесенки) |
| iziswap → IziSwap → OraclePmm | лесенка | dex.pmm_ladder_usd | неприменимо: каждым блоком (per_block.rs:57, пары с USDT или dex.pmm_ladder_counter) и на событиях — Refresh (events_izi.rs:31) | неприменимо |
| biswap_v3 → BiswapV3 → OraclePmm | лесенка | dex.pmm_ladder_usd | то же (per_block.rs:58, events_izi.rs:31) | неприменимо |
| woofi_v2 → WooPP → OraclePmm | лесенка (пара общего контракта) | dex.pmm_ladder_usd | неприменимо: каждым блоком (per_block.rs:52) и на логах контракта (per_block.rs on_member_log) | неприменимо |
| dodo_dvm / dodo_dpp / dodo_dsp → Dodo* → Pmm | вся кривая PMM (getPMMStateForCall: i, K, B, Q, B0, Q0, R — load_dodo.rs:32) | — | неприменимо: части нет; события → Refresh (events_dodo.rs:50-61) | неприменимо |
| pancakeswap_v2, uniswap_v2, v2_fork, v2_fork_executor → V2 → V2 | резервы (всё состояние) | — | неприменимо | неприменимо |
| solidly_v2 → Solidly, solidly_v1 → SolidlyV1 → Solidly | резервы, флаг stable, комиссия | — | неприменимо | неприменимо |
| curve_stableswap → CurveStable; pancake_stableswap → PancakeStable; lista_stableswap → ListaStable; saddle_stableswap → SaddleStable; curve_stableswap_ng → CurveNg; nomiswap_stable → NomiStable; pancake_infinity_stable → InfinityStable → StableSwap | все балансы, A, комиссия | — | неприменимо | неприменимо |
| kyber_dmm → KyberDmm → AmpV2 | реальные и виртуальные резервы | — | неприменимо | неприменимо |
| smardex → Smardex → Fictive | резервы, фиктивные резервы, средняя цена | — | неприменимо | неприменимо |
| balancer_v2 → BalancerV2 → VaultPool | все балансы пула | — | неприменимо | неприменимо |
| unsupported | пулы не берутся | — | — | — |

Итог: из 31 Proto частичное состояние держат V3 (PancakeV3, UniswapV3, Slipstream), Cl (InfinityCl, UniswapV4), Algebra
(Algebra, AlgebraIntegral), Bin (InfinityBin), LbBin (LbV21 с USDT), Maverick и лесенки (Metric, iZi, Biswap V3, WooFi).
Перечитывания по выходу за часть не было только у Bin и LbBin — их и закрывает решение 332. У Maverick и лесенок часть
строится заново на каждом событии / блоке. V3/CL — всегда; Algebra — под 320 (рекомендуется «да»).

## Что сделано (решение 332)

- `pools/off_map_bins.rs` (новый): `sides` — прочитанная стоимость каждой стороны от НОВОГО активного бина (бины полосы
  ниже / выше), той же оценкой `load_bin::usdt_value` и тем же встречным токеном, что у загрузчика (Infinity Bin — как
  `load.rs` ветка Bin: USDT = Y, если он token1; LB — как `load_lb.rs`: X/Y по `x_is_token0`, без USDT — не действует);
  сторона, прочитанная до конца пула (край 0 / 0xFFFFFF), не проверяется. `check`: хотя бы одна сторона меньше
  `dex.bin_reread_left_pct`% от `dex.bin_depth_usd` → перечитывать. `after`: запись журнала dex.pool и
  `refresh_event::refresh` (причина `off_map_bins`; класс `dex.event_refresh_urgency`, замер `log.pool_refresh_timing`).
- `pools/listen_log.rs`, ветка `Applied::Bins`: после перечитывания затронутых бинов (как раньше) — `off_map_bins::after`.
  Ошибка чтения идёт тем же путём, что у прочих перечитываний (пул недостоверен; StaleData — очередь повтора, решение 298).
  Пул в очереди повтора (`waiting`) — не читается (как раньше).
- Настройка `dex.bin_reread_left_pct` [DEX] — каталог `settings/catalog/dex_event.rs`, видна в «Настройках» (раздел
  «Контракты DEX», по id DEX).

**Почему проверка после перечитывания бинов, а не до:** точная стоимость стороны — после getBin затронутых бинов (Burn /
Withdrawn уменьшают прочитанное — это тоже «мало прочитано»). Цена — лишняя пачка getBin перед полным чтением на том
событии, которое его вызвало (редко).

**Защита от перечитывания на каждом событии.** (1) Сразу после чтения каждая сторона по построению не меньше
`dex.bin_depth_usd` (загрузчик читает до этого, `load_bin.rs:92`, `load_lb_bins.rs:81`) или прочитана до конца — повтора нет,
пока обмены/выводы не съедят больше (100 − процент)% прочитанного в эту сторону. (2) После полного чтения на блоке события
остальные события этого блока уже учтены (`registry.rs` has_applied: synced_block = блок, synced_log = None) — не чаще раза
на блок на пул (у LB Swap — по событию на каждый пройденный бин, все они после первого пропускаются).

**Одна настройка или по видам — решение:** одна настройка `dex.bin_reread_left_pct` с областью «DEX» (по id DEX, как
`dex.pmm_ladder_counter`). Почему: (а) дыра есть только у двух видов из бинов, у них одно правило полосы
(`dex.bin_depth_usd`) и одна оценка стоимости — разные имена ничего не дают; (б) область «DEX» включает её по одной DEX
(= по одному виду: Infinity Bin и LB — разные DEX), как просил Тигран; (в) V3/CL уже перечитываются всегда, Algebra — под
своей `dex.algebra_off_map_reread` (решение 320) — общая на «все виды» настройка дублировала бы 320 и меняла поведение V3.
Не задана — 1 в 1 как сейчас (проверка заканчивается на чтении настройки, к узлу не идёт).

**Не сделано и почему.** (1) `per_block_lb.rs` (LB каждым блоком) не проверяет — активный бин меняют только обмены, а у
глубокого пула Swap-события приходят и проверяются; у «спящего» (мелкого) пула событий обмена нет, он не котируется и
просыпается полным чтением на добавлении ликвидности (`admit.rs`). (2) `depth_sync.rs` (смена настроек на лету) — не
трогался: правило зависит от `dex.bin_depth_usd` и `dex.bin_reread_left_pct`, их нет в списке DEPTH; включение действует со
следующего события бинов пула. (3) `load_*.rs` и `session.rs` (зоны t152/t153) не менялись — вызывается существующее
`load::refresh` через `refresh_event.rs`.

## Тесты

- `tests/bsc_dex_off_map_bins_332.rs` — LB V2.1 на записи `tests/data/lb_v21_2026-11-15/probe.json` (пул USDC/USDT
  0xaec9…060c, все непустые бины). Полоса — тем же правилом, что `load_lb_bins.rs` band, при `dex.bin_depth_usd` = 3:
  8388611…8388617. Не задана — false всегда; 50%: как прочитано — false; +1 бин — вверх 1,587 ≥ 1,5 — false; +2 — 0,952 —
  true; край и за краем — true; −1 — false; −2 — true; полоса до конца — false; без USDT, без dex.bin_depth_usd, у другой
  DEX — false. Локально не собирался и не запускался (правило задачи).
- **Infinity Bin — тест ждёт записи.** Функция та же (`check`, ветка `PoolState::Bin`), записи пула Infinity Bin в
  `tests/data` нет. Промпт серверному ИИ (только чтение):

  > Только чтение, узел и бота не трогать. Запиши для одного пула PancakeSwap Infinity Bin с USDT из справочника бота на
  > одном блоке (номер блока записать): PoolKey/PoolId и binStep, `getSlot0(id)` у BinPoolManager, ВСЕ непустые бины
  > (`getNextNonEmptyBin` в обе стороны до края + `getBin` каждого), decimals обоих токенов; и один настоящий лог `Swap`
  > этого пула после этого блока, у которого activeId другой. Сырые ответы JSON-RPC — в
  > `tests/data/infinity_bin_2026-11-16/` с README (адреса, блок, источник).

- Сквозной путь `listen_log.rs` → `after` → `refresh_event::refresh` требует узла — не покрыт тестом (как и V3 off_map).

## Риски

- Нагрузка: на пул из бинов — одно полное чтение (getSlot0 / состояние и обход бинов до `dex.bin_depth_usd` в обе
  стороны: по 2 вызова на бин у Infinity Bin, у LB — тоже последовательно, `load_lb_bins.rs`) на событии, съевшем больше
  (100 − процент)% прочитанного. Совет — начать с одной DEX, смотреть журнал dex.pool «в сторону цены прочитано бинов меньше»
  и замер `log.pool_refresh_timing` (причина `off_map_bins`).
- Процент такой, что `dex.bin_depth_usd` × процент / 100 меньше `dex.min_pool_liquidity_usd`, — пул успеет стать «мелким»
  до перечитывания. Совет — в FIRST_RUN.
- Пара Infinity Bin с промежуточной монетой (не USDT): стоимость считается так же, как у загрузчика (`load.rs`: встречный
  токен записи как «USDT», его decimals) — правило согласовано с загрузчиком, но «USDT» там — единицы встречного токена.
  Это свойство загрузчика, не t154.
