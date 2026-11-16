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
| `admit.rs admit` (первая загрузка, `load_batch` через `load_many`) | пачку не рвёт: пул в очередь повтора (`admit.rs:57-72`) | пул на паузе 339 не читается — сразу в очередь повтора (число попыток из отметки), без записи журнала (раздел 10); остальное без изменений; `End` — только подписка / блок WS / узел HTTP до блока (`admit.rs:47-49, 83-88, 101, 118-120`) |
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

## 5. Решение 330: кто кого ждёт в сессии пачки (факт кода 8fe83aa1)

- Пачка пулов подписки — одна задача tokio `listen::run` → `session::run` → `listen` (`session.rs:87`): один цикл
  `tokio::select!` по веткам: лог (`g.rx.recv()`), новые пулы, пересинхрон ленты, новый блок, настройки.
- Ветка лога ждёт `on_log` до конца; для глубокого пула — `listen_log::on_log`, а тот — перечитывание по событию
  (`listen_log.rs:104` off_map, `:119` бины, `:141` Refresh). Пока идёт перечитывание, логи ВСЕХ пулов пачки лежат в
  канале `g.rx` (поле queued записи dex.pool_refresh — `refresh_event.rs:96`).
- Ветка нового блока так же ждёт `reload::retry` и `per_block::on_head` (пулы по одному, `per_block.rs:110-118`).

Стало:

| Путь | dex.event_lanes_max | dex.block_read_parallel_max |
| --- | --- | --- |
| Перечитывание по событию (off_map, Refresh, бины, реорг) и применение событий глубокого пула | полоса пула — отдельная задача; другие пулы идут (`session.rs` on_log → `session_lanes.rs`) | — |
| Пробуждение мелкого пула (`admit::wake`), пары Multi (`listen_site`), пары WooFi (`on_member_log`) | в сессии, как раньше (меняют подписку `g.set` и очередь повтора `&mut Members`); полосы других пулов при этом идут | пары WooFi — одной группой с пределом |
| Каждым блоком (`per_block::on_head`) | ждёт только полосы своих пулов (`settle_block`) | одной группой с пределом + Balancer / LB одновременно (`per_block_par.rs`) |
| Повтор (`reload::retry`), приём (`admit`), пересинхрон (`depth_sync`) → `load_many` | повтор — после полос своих пулов; приём / пересинхрон / настройки — после всех полос (`drain`) | виды «по одному» одновременно с пределом, каждый на своём свежем блоке (`reload_many.rs` at_fresh — блок берётся при начале чтения пула) |

Вид × «ожидание пачки»: события своих пулов (Contract / Singleton) всех видов — V2, PancakeV3, UniswapV3, InfinityCl,
UniswapV4, InfinityBin, LbV21, KyberElastic, Dodo*, Algebra, AlgebraIntegral, Solidly, SolidlyV1, Slipstream,
Curve/Pancake/Saddle/Lista/Nomi/InfinityStable, CurveNg, Maverick, MetricOmm, IziSwap, BiswapV3, KyberDmm, Smardex,
BalancerV2 — действует (лог находит пул по индексу пачки, `session.rs` on_log). WooPP (Member) и пары Multi
(Curve*/Pancake/Saddle/CurveNg/Balancer при dex.multi_pair) — неприменимо к полосам: обработка меняет общие для пачки
подписку и очередь повтора (`listen_site.rs` on_log(&mut Group, &mut Members), `admit.rs wake`); для них — пары
одной группой (dex.block_read_parallel_max). Каждым блоком — только виды `per_block::needs` (`per_block.rs:50-66`):
ListaStable, WooPP, InfinityStable, CurveNg, NomiStable, MetricOmm, IziSwap, BiswapV3, KyberDmm, Smardex, BalancerV2,
LbV21; остальные — неприменимо (их состояние меняют только события).

Порядок и гонки (полосы):
- Порядок одного пула: свой канал; следующее событие пула запускается только после завершения предыдущего
  (`lanes.rs next/done`). Событие, пришедшее во время перечитывания, ждёт в канале и применяется после — не теряется;
  уже учтённое перечитыванием пропускается по позиции (`listen_log.rs` has_applied).
- Двойное перечитывание одного пула полосами невозможно (у ключа не больше одной работы). Полоса и чтение блока одного
  пула: новый блок ждёт полосы пулов из очереди повтора и видов `per_block::needs` (`settle_block`), новые не
  запускает до конца чтения блока.
- Очередь повтора: «ждёт ли пул повтора» берётся при запуске события; итог `Ok(true)` (StaleData или решение 339)
  ставит пул в очередь при завершении — до следующего события пула. Пул на паузе 339 — `listen_log` сам не читает.
- Реорг (`removed: true`): событие идёт в полосу своего пула по порядку; перечитывание на текущем блоке — как раньше.
- Предел — dex.event_lanes_max (читается при каждом запуске; снята — принятое дорабатывается по одной, потом сессия
  снова по очереди). Узел: каждый запрос — общая очередь `node/pace.rs` (bsc.node_http_rps); класс перечитывания —
  dex.event_refresh_urgency (task_local на время чтения), задача полосы — без класса (фон, как задача пачки).
- Обрыв: JoinSet полос останавливается вместе с сессией; пулы — как раньше (недостоверны или dex.keep_pools_on_break),
  переподключение читает их заново.
- Остаётся: повтор и «каждым блоком» идут в ветке нового блока сессии — на это время новые логи в полосы не
  раздаются (сами чтения — одновременно); per_block одним Multicall3 не сделан (внутренности load_*.rs — t152).

## 6. Что не покрыто тестом и чего нет в записях

- `tests/bsc_dex_pool_fail_339.rs` — расписание паузы и независимость отметок пулов; каталог 339. Нет записи ответа
  KJUDGE_ERROR (промпт — docs/TASKS.md, 339).
- `tests/bsc_dex_session_lanes_330.rs` — расписание полос на 2305 реальных логах (квитанции 4 блоков BSC,
  `tests/data/receipts_2026-10-25`): порядок пула, предел, «медленный пул не держит других»; каталог.
- `tests/bsc_dex_read_parallel_331.rs` — каталог. Нет записи времени verify с настройкой и без (промпт — TASKS, 331).
- Сами пути сессии (задачи tokio, узел) тестом не прогоняются — нужен узел; замер dex.pool_refresh — TASKS, 330.

## 7. Что взято из черновика b21c1825 и что переделано

- Взято: `lanes.rs` (расписание; `next` переписан без get_mut в match), `session_lanes.rs`, правки `session.rs`,
  `load_batch_par.rs`, `per_block_par.rs`, правки `reload_many.rs`, `verify.rs`, `cycle_legs.rs`, `load_batch.rs`
  (Want: Copy), тест полос, строки настроек.
- Переделано: черновик не трогал главное (339) — сделано заново (`pool_fail.rs` и пути); per_block / пары WooFi /
  приём — номер решения 330 (не 331); `per_block_par::invalid` → `failed` (причина и пауза 339); в docs/SETTINGS.md у
  строк t153 не было столбцов «единица» и «область»; в `pools/mod.rs` черновика не хватало объявлений; коммит разбит
  на А / В / Б.

## 8. Правки listen_log.rs (для слияния с t152 / t154)

Только решение 339, три места (по коммиту А): шапка — строки 16-19 (две строки «Решение 339 …» и настройка в
«Настройки:»); комментарий on_log — строки 53-54; в `on_log` после `let Some(entry) = p.reg.get(&r)` — строки 78-79
(`let waiting = waiting || super::pool_fail::paused(p, &r);`); в ветке ошибки после `stale_read::note … return
Ok(true); }` — строки 165-168 (`if super::pool_fail::note(...).await { return Ok(true); }` перед прежним
`return Err(End::Broken(e.message))`). Ветка Bins (t154) и чтение (t152) не тронуты.


## 9. Независимое ревью ветки и правки (16.11.2026)

Каждое замечание перепроверено по коду efd9fd5d. Номера строк ревью для `session_lanes.rs` (240-242) и
`per_block_par.rs` (266-271) не совпадают с файлами (188 и 66 строк) — выводы по сути верные, места найдены по коду.

| Замечание | Факт кода (efd9fd5d) | Что сделано |
| --- | --- | --- |
| Б1 брошенный пул | `reload.rs:90-112` — после dex.pool_load_retries пул в `abandoned`; `listen_log.rs` — Applied::Updated без off_map не перечитывает; при 339 переподключений по ошибке пула нет | верно. При dex.pool_fail_pause_ms пул не бросается (кроме ждущего незаданную настройку), после предела — одна запись Error, повтор с паузой; событие недостоверного пула не на паузе, не перечитавшее его, — итог true → очередь повтора (`pool_fail::requeue`); пары Multi перечитываются любым логом, «каждым блоком» — каждым блоком после паузы |
| Н1 обрыв с полосами | JoinSet снимался drop при возврате из listen | верно (гонка возможна по коду). `session.rs listen` → `events`, затем `lanes.shutdown().await` до возврата в `run` (invalidate) |
| Н2 молчаливый сброс | `session_lanes.rs:88-91` — `if send.is_ok()` | верно. `push` → `End::Broken` |
| Н3 паника | `pool_fail.rs:94` `now + Duration` | верно. `pool_fail::later` — checked_add, пауза делится пополам |
| Н4 ошибки узла | `listen_log.rs` / `reload.rs` — любая ошибка → `note` на пул, сравнение причины по тексту | верно. Unreachable / Timeout / RateLimited (как `node/failover.rs switchable`) — пауза начальная, без роста; журнал одной записью на сбой узла; причина — по коду |
| Н5 замок без 339 | `registry.rs put` → `fails.clear` под Mutex | верно. Атомарный счётчик отметок |
| Н6 отметки не чистятся | карта только растёт | верно. `registry.rs retain` → `forget`, `market.rs` → `keep_only(known)` |
| Н7 Telegram | события нет | сделано: dex.pool_unread_alert_s, событие dex.pool_unread |
| Н8 queued | смысл разный при полосах и без | описано: SETTINGS (log.pool_refresh_timing), `refresh_timing.rs`, `session_lanes.rs`, DECISIONS |
| Н9 тесты | — | `tests/bsc_dex_review_t153.rs` |

Задача t160 (зона ветки): dex.per_block_skip_same — см. DECISIONS «339 (ревью, t153)». Без dex.pool_events_keep = да
не действует (решение 313: событие при полном канале теряется, повтор каждым блоком был единственной доставкой).

## 10. KJUDGE_ERROR на записях живого бота (16.11.2026, tests/data/dodo_kjudge_2026-11-16)

Записи: журнал (493 записи dex.pool с KJUDGE_ERROR за сутки) и сырые eth_call 5 пулов DODO на блоке 133993452.
Тест — tests/bsc_dex_dodo_kjudge_2026_11_16.rs. Исходники DODO — DODOEX/contractV2 2f1bcda.

**Какой вызов бота откатывается (факт кода + исходника).** Журнал пишут только приём пачки (admit.rs:57-65, Warn
«… не прочитано: … — повтор на новых блоках») и повтор (reload.rs:127-149, Error «так и не прочитано за 6
попыток … недостоверен до переподключения пачки»; текст — ветка без решения 339: на боте 339 не включено). Оба
читают пул через load.rs:74 → load_dodo.rs:32-42: getPMMStateForCall() → getUserFeeRate(0x0) → _BASE_TOKEN_ →
_QUOTE_TOKEN_. Строка KJUDGE_ERROR в исходниках — только FeeRateDIP3Impl.getFeeRate (L116-L124), куда ведёт
getUserFeeRate (DPPVault.sol L34-L41 → FeeRateModel.sol L28-L32); getPMMStateForCall её не вызывает (DPPStorage.sol
L51-L83; adjustedTarget при K = 0 не откатывает — DODOMath._SolveQuadraticFunctionForTarget L71-L73). Значит,
откатывается второй вызов — getUserFeeRate. Адрес пользователя (0x0) на условие не влияет: require стоит до любого
использования user (L119-L124). Тот же getFeeRate стоит в querySellBase / querySellQuote (DPPTrader.sol L215, L237;
DSPTrader.sol L208, L230; DVMTrader.sol L159, L174), а сделка sellBase / sellQuote вызывает querySell* (DPPTrader.sol
L52, L87) — пока у пула K = 0 и нулевой резерв, на цепи откатывается и обмен. Ошибка не в нашем вызове.

**Какие пулы (факт записи).** У всех 5 снятых пулов K ≠ 0 (DSP USDC 2e14, DPP BNB 3e17, DVM WBNB 1e18, DPP CAKE
1e18, DPP ETH 5e15), версии — из списка _kjudge, getUserFeeRate(0) ответил; mtFeeRate = feeRateImpl.getFeeRate(пул, 0)
у всех 5. KJUDGE давали другие пулы: адресов в журнале нет ни в одной записи; два адреса сервер взял из записей
dex.pool_refresh (это пулы с событиями, не те, что падают). Фабрика 0xd9CA… знает один DPP CAKE/USDT, а в журнале 4
разных DPP CAKE (одна Warn на пул в приёме) — минимум 3 из них с другой второй монетой.

**Что именно по кругу (факт записи).** 19 всплесков; в каждом по одной записи Warn и Error на пул, набор один и тот
же: 19 пулов до 17:49 (dodo-dpp USDC ×7, dodo-dpp CAKE ×4, dodo-dsp PORTO ×2, dodo-dsp USDC, dodo-dsp BNB, dodo-dpp
ETH, dodo-dpp BNB, dodo-dvm USDC, dodo-dvm BNB), с 17:49 ещё dodo-dpp DEXE и dodo-dpp 1INCH — 21. Всплески — 20:51,
10:25, 11:22, 15:22, 17:02, затем 17:17–18:00 каждые 1–6 мин (две группы: DSP — 4 пула, DVM+DPP — 15 пулов,
попеременно), 20:26. В 17:18 у 13 пулов DPP Warn без Error — приём оборвался до конца повторов. Записей на пул:
в среднем 2 на всплеск; в час 17:00–18:00 — 16 на пул DSP PORTO, 13 на пул DPP USDC. На узел: попытка пула = 2 eth_call
до отката (load_dodo.rs:33-40), 6 попыток на всплеск. Почему пачки переподключались — в записях нет (link.down не
найден); промпт — TASKS t153 (б), п. 4.

**Мёртвые ли пулы.** По исходнику — да: при K = 0 и нулевом резерве пул не торгуется на цепи вообще (см. выше), значит
сделок мы не теряем. НЕ доказано на цепи: (1) K и резервы именно откатывающихся пулов не сняты; (2) развёрнутый
feeRateImpl 0xe5e9… на version() откатывается без данных, а у FeeRateDIP3Impl 2f1bcda version() = «1.2.0»
(L221-L223) — код на BSC не тот же, совпадает только текст ошибки. Промпт — TASKS t153 (б), п. 1-3.

**Что будет с ними на ветке (факт кода).** Ответ «execution reverted …» → SwapReverted (errors.rs from_rpc), не связь
(pool_fail.rs is_link) → при dex.pool_fail_pause_ms пауза растёт вдвое до dex.pool_fail_pause_max_ms (pool_fail.rs
record / pause_after), журнал — на первую неудачу и смену кода (pool_fail_note.rs note). После dex.pool_load_retries
пул не бросается (reload.rs:109-126, ревью Б1): одна запись Error, повтор идёт дальше с паузой; пачку не рвёт. Когда
ликвидность вернётся — ближайший повтор (не позже потолка паузы) прочитает пул, put снимет отметку, пул снова в
торговле. Это важно для DPP: пополнение владельцем reset() пишет только LpFeeRateChange (DPPVault.sol L123-L147), а
бот его не слушает (events_dodo.rs topics) — без повтора с паузой пул вернулся бы только переподключением. Было: каждый
приём пачки читал все свои пулы без учёта паузы — по записи Warn на мёртвый пул на каждое переподключение; сделано
(DECISIONS «339 (журнал и приём, t153)»): пул на паузе при приёме не читается — сразу в очередь повтора с числом
попыток из отметки, без записи. Пул DPP ETH 0x238b… с резервами 0/0 и K ≠ 0 — читается без
ошибки, математика его не котирует (swap_dodo.rs → None), в сделки не идёт.

**Ошибка в коде чтения DODO — не найдена.** Правка не нужна. Сделано по разбору (не ошибка): адрес пула в записях
журнала о непрочитанном пуле и в уведомлении dex.pool_unread (depth_book::label, как у dex.pool_refresh) — все виды.

## 11. Досъёмка: 28 пулов DODO, которые бот не читает (блок 133995628, tests/data/dodo_kjudge_2026-11-16_2)

Тест — tests/bsc_dex_dodo_dead_pools_2026_11_16.rs. Пулы — по фабрикам dex.factory видов DODO для USDC, CAKE, PORTO,
BNB, ETH, DEXE, 1INCH × (USDT и 16 адресов dex.intermediate_tokens): 328; в /api/coins бота 300 (у всех ответы без
отката чтения), 28 — нет. «Нет в /api/coins» = бот ни разу их не прочитал: экран «Монеты» строится по реестру
(depth_facts.rs all — reg.refs()), а в реестр пул попадает только после удачного чтения. В журнале бота они есть
(«Новый пул …» в окнах запуска; KJUDGE_ERROR / ORACLE_NOT_FEASIBLE при приёме пачек).

**21 пул KJUDGE_ERROR — мёртвые (факт записи).** Это ровно набор из журнала (dodo-dpp USDC ×7, CAKE ×4, dodo-dsp
PORTO ×2, по одному dodo-dvm USDC / BNB, dodo-dpp ETH / BNB / DEXE / 1INCH, dodo-dsp USDC / BNB). У всех K = 0 и хотя
бы один резерв 0: у 9 — оба 0; у остальных второй резерв — пыль (наибольший 934700000277593571 ед. USDC-пула
0x9327…, ≈ 0,93 при 18 знаках; прочие 11 — 4e16, 4.39e16, 1e16, 1.93e14, 1e14, 1e8, 11055942, 249016, 1001,
57, 56 ед.). По всем
328 пулам: откат KJUDGE ⇔ «K = 0 и нулевой резерв» (21 из 21, ложных 0). У всех 21 обе котировки откатываются:
querySellBase / querySellQuote — KJUDGE_ERROR (сторона с резервом) или TARGET_IS_ZERO (пустая сторона; DODOMath.sol
L43 / L129 require V0 > 0). Обмен DODO идёт через querySell* (DPPTrader.sol L52, L87) — на цепи этими пулами
торговать нельзя ни в одну сторону. Сделок не теряем.

**Откуда KJUDGE_ERROR.** В байткоде feeRateImpl 0xe5e9… (6210 байт) строки «KJUDGE_ERROR» нет; в нём есть строки
«DVM 1.0.3», «DSP 1.0.1», «DODOFeeImpl: EXCEED_YOUR_QUOTA» и селекторы _K_(), _BASE_RESERVE_(), _QUOTE_RESERVE_(),
version(), isPoolHeartBeatLive(address), _LP_FEE_RATE_() (разбор PUSH4 байткода). Значит, require с этим текстом
стоит в другом контракте, который вызывает feeRateImpl (адреса в байткоде нет — берётся из хранилища); какой — по
записи не установлено (нужен eth_getStorageAt / трассировка вызова). Для нас это не важно: условие подтверждено
на 328 пулах, и квоты пула откатываются сами.

**6 пулов ORACLE_NOT_FEASIBLE** (0x39a5d6ce ETH/USDC, 0xfeafe253, 0x5928e8f3, 0x05d968b7 — WBNB/USDT, 0x9ad32e30,
0x1dc7dcf8 — ETH/USDT): version() = «DPP Oracle 1.1.0», K = 1e14 (у 0x1dc7… 1e12), резервы 0/0; откатываются
getPMMStateForCall и обе котировки. Исходника «DPP Oracle» и строки ORACLE_NOT_FEASIBLE в DODOEX/contractV2 нет (все
ветки, git grep) — что значит ошибка по исходнику, не установлено. Факт записи: пулы пустые и не котируются на цепи —
сделок не теряем. Открыто: верна ли наша математика для «DPP Oracle», когда у такого пула появятся резервы и цена
(в /api/coins таких пулов нет — проверить не на чем) — TASKS.

**1 пул DVM 1.0.0** (0x0ab58740…, в журнале «dodo-dvm DAI»; монеты 0xb8b73763… / 0xd67e71b2…): K = 0, оба резерва
2^104 − 1, i = 1; getUserFeeRate и обе котировки — откат без данных («execution reverted: 0x»). Не котируется на цепи.
Исходник DVM 1.0.0 — только в ветке feature/V2 contractV2 (не 2f1bcda).

**Как бот с ними (факт кода).** Все три вида ошибок — «execution reverted …» → SwapReverted (errors.rs from_rpc), не
сбой связи. Тест на записанных ответах: load_dodo.rs читает рабочий пул и даёт SwapReverted у KJUDGE, оракула и
DVM 1.0.0 (откат на 2-м вызове getUserFeeRate у KJUDGE и DVM 1.0.0, на 1-м getPMMStateForCall у оракула). Без
решения 339 (так сейчас на сервере) — 6 попыток, «брошен до переподключения», при каждом приёме пачки снова 2 записи.
С 339 (ветка): одна запись на первую неудачу эпизода (pool_fail::note — и при приёме, DECISIONS «339 (журнал и
приём)»), одна Error на пределе попыток, дальше повтор с паузой до потолка; пул на паузе при переподключении не
читается; появится ликвидность (или цена оракула) — ближайший повтор прочитает пул и вернёт его в торговлю.
Отдельный признак «мёртвый пул» не нужен: та же пауза 339 ограничивает и запись, и нагрузку (2 eth_call на пул не
чаще потолка паузы).

**Всплески (факт записи pre_burst_summary.json; всплеск = записи admit.rs — приём пачки).** 20 всплесков, 539
записей: 4 окна — запуск бота / новые пулы (684–4857 записей «Новый пул» за 30 с: 10:25, 15:22, 17:02, 17:49); 3 —
узел отставал (venue.head_age «Голова сети устарела … снова свежая»: 20:26, 20:41, 20:52); 13 (11:22, 17:17–18:00) —
записи о причине нет. По коду приём пачки — только начало сессии или новые пулы (session.rs:72, :163), т. е. это
переподключения; в 17:18:47 у 13 пулов DPP есть Warn без Error — сессия кончилась раньше 5 повторов. Запись об обрыве
(venue.ws «Связь с узлом BSC (WebSocket) потеряна: причина») пишется одна на обрыв, пока не восстановятся ВСЕ пачки
(link.rs:29-45, up — только после удачного приёма) — её отсутствие в окне не доказывает, что обрыва не было, а причины
следующих обрывов не видны вовсе. Причина 13 переподключений по записям не установлена; промпт — TASKS t153 (в).

## 12. Экран непрочитанных пулов, запись каждого обрыва пачки, неизвестные версии DODO (DECISIONS «339 (экран и журнал, t153)»)

**Экран «Монеты» (сделано, все виды пулов, без настройки).** Было: экран строится по реестру (depth_facts.rs all —
reg.refs()), в реестр пул попадает только после удачного чтения — 28 пулов DODO из раздела 11 не были видны вовсе.
Стало: неудача чтения пула без записи в реестре (admit.rs unread — приём пачки и reload.rs retry) ставит отметку
pools/pool_unread.rs (монета, второй токен, вид, комиссия из справочника, причина); любая запись пула в реестр
(registry.rs put) и чистка по найденным (market.rs keep_only) её снимают. depth_facts.rs отдаёт такой пул отдельным
PoolDepth с полем `unread` (core/types/pool_depth.rs, стык — новое необязательное поле, в JSON только у
непрочитанного), valid = false, причина «пул ещё ни разу не прочитан — в котировки и сделки не идёт», в details —
«не прочитан: причина …» с паузой 339 или прежним повтором. api/coins.rs with_depths кладёт его в строку монеты (по
адресу контракта) пулом с `unread: true` и `admission` = та же причина; интерфейс не менялся (CoinRow /
PoolDepthCell уже выводят admission, depth_error, valid, details). Ядро и расчёты эти пулы не видят (market не трогается).

**Каждый обрыв пачки — в журнал (сделано).** link.rs down: запись venue.ws (Warn) «Пачка пулов {имя} ({N} пулов):
сессия подписки прервана — {причина}…» на каждый обрыв; одинаковые (та же пачка, причина без чисел и слов с 0x) — не
чаще log.pool_break_interval_s, пропущенные называются числом в следующей (link.rs due / shape); не задана — каждый
обрыв. Прежние «потеряна / восстановлена» — как были.

**Неизвестные версии пулов DODO («DPP Oracle 1.1.0», «DVM 1.0.0») — защита уже стоит на пути (факт кода), правки нет.**
- Бот version() у DODO не читает (load_dodo.rs:32-42). Версию читает только Curve NG — и не пускает версии вне
  dex.curve_ng_versions (pools/verify_curve_ng.rs:121-130).
- Любая сторона любого пула идёт в котировку и сделку только с прошедшей сверкой своей стороны с квотером:
  quote/select.rs:101-107 («сторона пула ещё не сверена с квотером» / причина расхождения). У DODO квотер — сам пул:
  querySellBase / querySellQuote (quote/quoter_calls.rs:98-127), т. е. тот же контракт и та же функция, что считает
  обмен на цепи (DPPTrader.sol L52, L87). Сверка — на одном блоке, расхождение больше dex.quoter_max_diff_pct — сторона
  на паузе (check/quoter_cycle.rs:2-7).
- select::usable стоит на всех путях: прямой маршрут (quote/direct.rs:53), умный (quote/smart_ways.rs:42), через
  промежуточную (quote/via_pools.rs:39), граф (quote/graph_legs.rs:90), циклы DEX ↔ DEX (quote/cycle.rs:109,
  quote/cycle_legs.rs:148), проверка перед сделкой (quote/verify_decide.rs:176), исполнение (exec/live.rs:47,
  exec/live_hops.rs:70).
- Пустой пул (как все 6 «DPP Oracle» сейчас) отсекается ещё раньше — quote/select.rs:72 (quote/emptiness.rs).
Значит, пул DODO неизвестной версии с неверной для него математикой не котируется: сверка с его же querySell* не
пройдёт. Не установлено: верна ли математика для «DPP Oracle» вообще — проверить можно будет, когда у такого пула
появятся резервы (сейчас все пустые); до этого он не торгуется по факту кода выше.
