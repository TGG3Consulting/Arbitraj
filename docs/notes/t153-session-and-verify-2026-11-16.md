# t153 — ошибка одного пула, сессия пачки, перечтение маршрута (16.11.2026)

Ветка `t153/session-verify-parallel` от `rel/dex-all` 8fe83aa1. Решения 339 (ошибка одного пула не рвёт пачку),
330 (пачка не ждёт), 331 (проверка маршрута параллельно). Строки «было» — по 8fe83aa1, «стало» — по коммитам задачи.
Локально не собиралось и не запускалось (правило задачи) — проверка в GitHub (CI: fmt, clippy -D warnings, тесты).
Начато другим агентом (черновик b21c1825, заменён коммитами задачи; что взято — раздел 7).

## 1. Решение 339: где ошибка одного пула трогала всю пачку (факт кода 8fe83aa1)

| Место | Было | Стало при dex.pool_fail_pause_ms |
| --- | --- | --- |
| `listen_log.rs:151-160` — перечитывание по событию (off_map, Refresh, бины, eth_blockNumber реорга) | любая ошибка, кроме StaleData → `End::Broken` → вся пачка переподключается и перечитывает все пулы (`admit`) | пул недостоверен, Invalidated, отметка с причиной и паузой (`pool_fail::note`), `Ok(true)` → очередь повтора (`session.rs` requeue); сессия идёт дальше |
| `listen_site.rs:121` — eth_blockNumber реорга у пар многомонетного пула | `End::Broken` | все перечитываемые пары недостоверны, Invalidated, отметка, очередь повтора |
| `listen_site.rs:135-140` — чтение пар многомонетного пула | ошибка (не StaleData) хоть у одной пары → все пары недостоверны и `End::Broken` | каждая упавшая пара отдельно: недостоверна, отметка, очередь повтора; прочитанные пары остаются |
| `reload.rs retry` → брошенный пул (`abandoned`) | брошен «до переподключения»; его событие перечитывает его (`listen_log`, waiting = false), ошибка → `End::Broken` → переподключение → `admit` снова читает все пулы → снова 6 попыток | событие брошенного пула на паузе не читается (позиция отмечается, `listen_log.rs` waiting); после паузы — одно чтение, ошибка → снова в очередь с растущей паузой, сессия не рвётся |
| `reload.rs retry` — очередь повтора | каждый блок, до dex.pool_load_retries | пул на паузе не читается и попытку не тратит; прочая неудача удлиняет паузу (StaleData — без паузы) |
| `per_block.rs on_head / on_member_log`, `per_block_lb.rs`, `per_block_balancer.rs` | пачку не рвут: пул `valid = false` без причины в журнале, следующий блок читает снова (постоянная ошибка — каждым блоком вечно) | то же + отметка с причиной (экран, журнал) и пауза: пул на паузе каждым блоком и по логу WooFi не читается |
| `admit.rs admit` (первая загрузка, `load_batch` через `load_many`) | пачку не рвёт: пул в очередь повтора (`admit.rs:57-72`) | без изменений; `End` — только подписка / блок WS / узел HTTP до блока (`admit.rs:47-49, 83-88, 101, 118-120`) |
| `admit.rs wake` | ошибка чтения — запись в журнал, пул остаётся мелким | без изменений; `End` — только подписка / блок WS / узел HTTP |
| `depth_sync.rs resync / on_change` | пул в очередь повтора | без изменений; `End` — только подписка / блок WS |
| `load_batch.rs` ступени Multicall3 | откат вызова пула — только этот пул (`load_batch.rs:7`, `try_chunks`); сбой части пачки — пулы этой части (сбой узла, не пула) | без изменений |
| Пары `PoolRef::Multi` (`load_site.rs`) | одно чтение на контракт — его ошибка общая для всех пар контракта (один контракт) | без изменений по смыслу; дальше — как строка listen_site |
| `check/*` (сверка с квотером, налоги, крючки) | ошибка пишется в итог пула (`check/state.rs PoolFacts`), сессию пачки не трогает | неприменимо |
| `quote/verify.rs`, `quote/cycle_legs.rs` | ошибка пула маршрута — отказ этого маршрута | неприменимо (это отказ сделки, не пачки) |

Остаются переподключением (обрыв, связь, порядок): канал подписки закрыт / Msg::Closed (`session.rs:108`), лог не
разобран (`session.rs:112`), лог без blockNumber/logIndex (`listen_log.rs:76`, `listen_site.rs:45`, `session.rs:239`),
лента остановлена / пропуск блока лентой — пересинхрон без разрыва (`session.rs:128-139`), поток блоков остановлен,
смена bsc.pool_events_source, ошибки подписки `g.set / g.block / g.release` и узла HTTP до блока (`admit.rs`,
`depth_sync.rs`), ядро закрыло канал (`End::Closed`).

Журнал: одна запись dex.pool на начало неудач пула и на смену причины («… не прочитано: причина — пул недостоверен …,
следующее чтение не раньше чем через N мс … (решение 339)»); прежние записи reload.rs «прочитано с попытки N» /
«так и не прочитано за N попыток» остаются. Экран «Монеты»: `depth_facts.rs` → `PoolDepth.details` строка
«не прочитан: причина (неудач подряд N; следующее чтение не раньше чем через N мс)» под «данные пула недостоверны»
(`ui/src/components/PoolDepthCell.svelte:43,46` — существующий путь `venue.rs pool_depths` → `api/coins.rs`).
Отметка снимается при записи достоверного состояния (`registry.rs put`, valid = true — все 6 мест записи после чтения).

## 2. DODO «KJUDGE_ERROR» (официальные исходники DODOEX/contractV2, HEAD 2f1bcda — тот же, что у решения 23)

- `load_dodo.rs` читает по очереди: `getPMMStateForCall()`, `getUserFeeRate(address(0))`, `_BASE_TOKEN_`, `_QUOTE_TOKEN_`.
- `getUserFeeRate(user)` у DVM/DPP/DSP: `mtFeeRate = _MT_FEE_RATE_MODEL_.getFeeRate(user)` (DVMVault.sol L38-L41,
  DPPVault.sol L34-L41, DSPVault.sol L39-L46) → `FeeRateModel.getFeeRate(trader)` = `IFeeRateImpl(feeRateImpl)
  .getFeeRate(msg.sender, trader)` (contracts/lib/FeeRateModel.sol L28-L32).
- Строка «KJUDGE_ERROR» есть в исходниках только в `contracts/DODOFee/FeeRateDIP3Impl.sol` L116-L124: для пулов с
  version() = «DVM 1.0.2», «DSP 1.0.1», «DPP 1.0.0», «DPP Advanced 1.0.0» (`_kjudge`, L217-L219)
  `require(!(k==0 && (baseReserve ==0 || quoteReserve == 0)), "KJUDGE_ERROR")` — откат, когда у пула K = 0 и один из
  резервов равен нулю.
- Тот же вызов `_MT_FEE_RATE_MODEL_.getFeeRate(trader)` стоит в querySellBase / querySellQuote (DVMTrader.sol
  L150-L177) — пока пул в таком состоянии, у него откатывается и котировка, т. е. ошибка не наша и постоянная до
  пополнения пула. Чинить чтение не нужно — нужно, чтобы этот пул не рвал пачку (решение 339).
- НЕ установлено по коду: что на BSC у этих пулов `feeRateImpl` — именно FeeRateDIP3Impl, их version(), K и резервы;
  и что вызывало переподключения пачки на живом боте (по коду возможен круг из строки 4 таблицы, но причина каждого
  обрыва — в журнале link.down). Промпт серверному ИИ — docs/TASKS.md, t153 (а).

## 3. Вид пула × путь — решение 339

Решение 339 стоит на путях, а не на видах: ошибка любого вида проходит через одни и те же места (таблица 1).

| Вид (Proto) | Ссылка | Ошибка по событию | Каждым блоком | Очередь повтора / брошенный |
| --- | --- | --- | --- | --- |
| V2, PancakeV3, UniswapV3, Solidly, SolidlyV1, Slipstream, Algebra, AlgebraIntegral, KyberElastic, DodoDvm, DodoDpp, DodoDsp, Maverick | Contract | действует (`listen_log.rs:165-168`) | неприменимо (`per_block::needs` — нет, `per_block.rs:50-66`) | действует (`reload.rs:57-80`) |
| InfinityCl, UniswapV4, InfinityBin | Singleton | действует (то же; бины — `reread_bins` в том же `res`) | неприменимо (needs) | действует |
| LbV21 | Contract | действует (бины `load_lb_bins` в том же `res`) | действует (`per_block_lb.rs:35-38, 66-79`) | действует |
| CurveStable, PancakeStable, SaddleStable | Contract; Multi (dex.multi_pair) | Contract — действует; Multi — `listen_site.rs:121-170` | неприменимо (needs) | действует |
| ListaStable, NomiStable, InfinityStable (Singleton), MetricOmm, IziSwap, BiswapV3, KyberDmm, Smardex | Contract / Singleton | действует | действует (`per_block.rs:96-127`) | действует |
| CurveNg | Contract; Multi | Contract — действует; Multi — listen_site | действует (Multi — `per_block.rs:128-134`) | действует |
| WooPP | Member | неприменимо к listen_log: лог контракта → `per_block::on_member_log` (`per_block.rs:152-166`) — действует там | действует | действует |
| BalancerV2 | Contract; Multi | Contract — действует; Multi — listen_site | действует (`per_block_balancer.rs:46-49, 71-76`) | действует |

## 4. Решение 331: проверка маршрута перед сделкой (факт кода 8fe83aa1)

- `quote/verify.rs:117` и `quote/cycle_legs.rs:66` читали пулы через `load_batch::states`: группы пар Multi по очереди
  (`load_batch.rs:85-91`), виды «по одному» по очереди (`load_batch.rs:98-117`, список — `one_by_one`,
  `load_batch.rs:232-249`), затем ступени Multicall3 V2/V3/CL/V4. Комментарий «один Multicall3-батч» был верен только
  для V2/V3/CL/V4 — исправлен.
- Стало (verify.read_parallel_max): `load_batch_par::states` — группы Multi и виды «по одному» потоком future с
  `buffered(n)` (не больше n сразу, порядок ответов сохраняется) и одновременно (`tokio::join!`) — ступени Multicall3 на
  подмножестве остальных пулов (`load_batch::states`). Те же функции на том же блоке → тот же итог; без dex.load_batch —
  та же причина у всех пулов без запросов к узлу. Чтения идут в задаче вызывающего — класс Trade сделки
  (task_local gateway/urgency.rs) действует на все запросы; темп — общая очередь node/pace.rs (bsc.node_http_rps).
- Вид × «проверка перед сделкой»: V2, PancakeV3, UniswapV3, InfinityCl, UniswapV4 — неприменимо (уже ступени
  Multicall3; теперь идут одновременно с остальными); все прочие виды (InfinityBin, LbV21, KyberElastic, Dodo*,
  Algebra, AlgebraIntegral, Solidly, SolidlyV1, Slipstream, стейблсвапы, WooPP, Maverick, MetricOmm, IziSwap,
  BiswapV3, KyberDmm, Smardex, BalancerV2) — действует (`load_batch.rs one_by_one`); пары Multi — одно чтение на
  контракт, группы контрактов одновременно.

