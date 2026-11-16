# t152 — перечитывание пула ПО СОБЫТИЮ у всех видов DEX (bsc_dex), 16.11.2026

Решение 325. Ветка `t152/event-reread-all-kinds` от `rel/dex-all` (8fe83aa1). Только факты кода — ссылки «файл:строка»
даны по этой ветке; цифры «до» с живого бота — из постановки задачи (записи dex.pool_refresh, решение 313).

## 1. Что вызывает перечитывание по событию

- `pools/listen_log.rs` `on_log`: `Applied::Refresh(why)` (реорг `removed`, событие не сходится с картой / не разобрано,
  событие ликвидности у видов без точечного применения) → `refresh_event::refresh`; `Applied::Updated`, если полоса цены
  вышла за карту (`depth::off_map`, `depth.rs:97-127`: только состояния V3, Cl и — при включённом off_map_algebra —
  Algebra) → `refresh_event::off_map`; `Applied::Bins(ids)` → перечитать бины (теперь `reread_bins.rs`).
- Чтение — `load::refresh` → `load::state` по виду (`load.rs`). Решение 315 — до него `load_v3_once::read` (только V3 с
  ABI PancakeSwap V3 / Uniswap V3).
- Класс очереди узла — `dex.event_refresh_urgency` (решение 314) у всех трёх путей (`refresh_event.rs` refresh,
  `refresh_urgency::urgent` у бинов и теперь у «только комиссии»).
- Каждое `call::view` — отдельный eth_call (отдельное место очереди темпа узла); `call::view_many` / `try_many` при
  `dex.multicall3` — один eth_call (`node/call.rs`, `node/multicall.rs`).

## 2. Таблица видов (обращений к узлу ПОДРЯД при перечитывании по событию)

«До» — по коду (`load*.rs`), при заданном dex.multicall3. «После» — при `dex.event_refresh_batch = да` для DEX (решение
325); «вызовы прошлого раза» — `prefetch_memo.rs` (есть со второго перечитывания пула).

| Вид / семейство | Что вызывает | До: обращений подряд и от чего зависит | Путь одним обращением до t152 | После (325) |
| --- | --- | --- | --- | --- |
| V2 и форки (`load_v2v3.rs:31`) | Refresh | 1 (getReserves) | уже 1 | 1 |
| V3 PancakeSwap/Uniswap ABI (`load_v2v3.rs:50/54,59,91`; [274 `fee_override.rs:98`], [281 `fee_fresh.rs:93`]) | off_map, Refresh | 3–5 | 315: 1–2 (`dex.event_refresh_one_call`) | 315 раньше; без неё — 1 (голова + слова вокруг тика события), тик ушёл из слов — 2 |
| Slipstream (`load_slipstream.rs:45,46,53,63`) | off_map, Refresh | 4 | нет | 1–2 (голова + слова; тики по битам), тик ушёл — 3 |
| CL: Infinity CL, Uniswap V4 (`load_cl.rs:50-51 / 54-61, 70 / 102, 81 / 113`) | off_map, Refresh | 4 | нет | 1–2, тик ушёл — 3 |
| Algebra V1.9, THENA Fusion (`load_algebra.rs:43,44,62,73`) | Refresh (Mint/Burn), off_map (при off_map_algebra) | 4 | нет | 1–2, тик ушёл — 3 |
| Algebra Integral: thena-v3, dexswap (`load_algebra_integral.rs:41-57`, обход `walk:100-121`) | Refresh (Mint/Burn, Burn 1.2.x), off_map; Fee / SwapFee / Swap 1.2.x с добавкой плагина — тоже полный Refresh | 4–5 + до 2 × dex.integral_tick_steps | нет | событие только комиссии — 1 (`dex.event_refresh_fee_only`); полное — 1–2, лист дерева не прочитан — 3 |
| KyberSwap Elastic (`load_elastic.rs:43,44,45,55,76`) | Refresh (off_map не применяется — состояние Elastic, `depth.rs:105-108`) | 5 | нет | 1–2 (голова + список; ticks()) |
| DODO PMM (`load_dodo.rs:33-42`) | Refresh на каждое событие кривой, в т. ч. DODOSwap (`events_dodo.rs:50`) | 4 (два из них — _BASE_TOKEN_, _QUOTE_TOKEN_) | нет | 1 (все четыре одним eth_call) |
| Infinity Bin — бины события (`reread_bins.rs`, было `listen_log.rs`) | Bins | по одному getBin на бин подряд | нет (у LB уже пачка, ревью 290) | 1 (`load_bin::many`, `node/call.rs` view_many) |
| Infinity Bin — полное (`load_bin.rs:58,75,122`) | Refresh | 2 + 2 на каждый пройденный бин (getNextNonEmptyBin → getBin, следующий id — из ответа) | нет | вызовы прошлого раза одним обращением; новые бины — по одному (цепочка) |
| Liquidity Book V2.1 (`load_lb.rs:116`, полоса `load_lb_bins.rs:62,97`) | Refresh; Bins — уже пачкой | 1 + цепочка бинов полосы | бины события — уже 1 | как Infinity Bin (вызовы прошлого раза) |
| Solidly / Solidly V1 (`load_solidly.rs:40,69/72`, `solidly_fee_read.rs`, `solidly_gate_read.rs`) | Refresh | 2–4 | нет | со 2-го раза 1 (вызовы прошлого раза; `solidly_*_read` смотрят в книгу) |
| Curve NG (`load_curve_ng.rs:46,93`) | Refresh | 2 (N_COINS → пачка) | нет | со 2-го раза 1 |
| Curve/Ellipsis, Saddle, PancakeSwap/Lista Stable, Infinity Stable, Nomiswap Stable, DMM (`try_many` / один view) | Refresh | 1 (decimals — из памяти `tokens.rs:126-146`) | уже 1 | 1 |
| SmarDex (`load_smardex.rs:47,88`) | Refresh | 1 (+ время блока без Multicall3) | 1 | 1 |
| Balancer V2 (`load_balancer.rs:60`, `load_balancer_fixed.rs:66-87` — один раз на пул) | Refresh | 1 (после первого раза) | 1 | 1 |
| Maverick V2 (`load_maverick.rs:122` try_chunks) | Refresh | частей по dex.load_batch | — | вызовы прошлого раза, частями по dex.load_batch |
| Metric OMM, iZiSwap, Biswap V3 (`load_metric.rs:47`, `load_izi.rs:49` + лесенка `load_metric_ladder.rs:104-111`) | Refresh | 1 + 2 прохода лесенки (пачка JSON-RPC с state override виртуального квотера) | — | голова из книги; лесенка — как раньше (см. п. 4) |
| WooFi (`load_woo.rs:53,86` + `load_woo_ladder.rs:109`) | Refresh | 1 + eth_getStorageAt + 2 прохода лесенки | — | try_many-вызовы (голова, лесенка) — из книги; eth_getStorageAt — как раньше (не eth_call) |

## 3. Algebra Integral: откуда 16–65 шагов и что такое words 13 / 3108 / 6932

- Шаги (каждый `call::view` — одно обращение): `load_algebra_integral.rs:41` globalState, `:42` liquidity, `:44` fee()
  (только при флаге DYNAMIC_FEE), `:49` nextTickGlobal, `:52` prevTickGlobal — 4 или 5; затем `walk` вверх (`:56`) и вниз
  (`:57`): цикл `for _ in 0..steps.max(1)` (`:100`), в каждом шаге `call::view(ticks(t))` (`:101`), следующий тик — поле
  `nextTick` / `prevTick` ЭТОГО ответа (`:121`); обход стороны кончается раньше, если дошёл до конца списка (MAX_TICK /
  MIN_TICK). Итого steps = 4 или 5 + (тиков вверх) + (тиков вниз), каждая сторона ≤ dex.integral_tick_steps. Значение
  dex.integral_tick_steps на живом боте в фактах задачи не дано — поэтому разложить 16/35/48/65 по сторонам не берусь.
- Пачкой обход как есть сделать нельзя — подтверждаю по коду: адрес следующего вызова (`ticks(n)`) есть только в ответе
  предыдущего (`:121`). Но порядок списка известен заранее по дереву поиска пула: тики списка — ровно установленные
  биты дерева (TickTree.sol; лист переключается вместе со вставкой/удалением тика — TickStructure.sol
  `_addOrRemoveTick` L36-L64, теги v1.2-integral / v1.2.1-integral и 1.2.10). Решение 325 читает второй слой дерева
  (28 узлов на весь диапазон) и нужные листы tickTable — и знает всю цепочку обхода без обращений по одному.
- words = `t.words.1 − t.words.0 + 1` (`refresh_event.rs:93-97`), окно — `window(down, up)` (`load_algebra_integral.rs:136-147`):
  дошли до конца списка вниз — первое слово `MIN_TICK.div_euclid(256)` = −3466; вверх — `MAX_TICK.div_euclid(256)` = 3465.
  - **6932** = 3465 − (−3466) + 1 — обход ДОШЁЛ ДО ОБОИХ КОНЦОВ списка (у пула мало тиков; окно — весь диапазон).
  - **3108** — дошёл до конца с одной стороны (вверх: последнее слово 3465, первое 358; или вниз: первое −3466,
    последнее −359), с другой — нет.
  - **13** — ни до одного конца не дошёл: 13 полных слов внутри прочитанного отрезка [нижний; верхний].
- Отдельно: Fee, SwapFee и Swap 1.2.0/1.2.1 с overrideFee = 0 и pluginFee ≠ 0 давали полный Refresh
  (`events_algebra_integral.rs`, было `Applied::Refresh`), хотя неизвестна только комиссия — теперь `Applied::Fee`
  (решение 325, `dex.event_refresh_fee_only`).

## 4. Где одним обращением нельзя (и почему)

- Цепочки, где адрес следующего вызова — в ответе предыдущего: обход бинов Bin / LB (`load_bin.rs:75`,
  `load_lb_bins.rs:62`) — дерева бинов у пула через view нет в нашем ABI; подсказка — вызовы прошлого раза (что не
  изменилось — из книги, новые бины — по одному). Integral — решено деревом (п. 3).
- Слова → тики (V3 TickLens нет: CL, Slipstream, Algebra V1.9): данные тиков зависят от битов слов, биты — от тика головы.
  Слова читаются заранее вокруг тика события (запись после применения события), тики — по битам: 2 обращения; если тик
  на блоке чтения ушёл из этих слов — 3. С вызовами прошлого раза (биты не менялись) — 1.
- Лесенки Metric / iZi (`load_metric_ladder.rs:104-111`): eth_call к виртуальному квотеру с state override (код квотера
  подставляется в вызов) пачкой JSON-RPC; в Multicall3 без state override не уходит, второй проход зависит от первого.
  Не трогал — отдельная задача.
- WooFi: eth_getStorageAt (`load_woo.rs:86`) — не eth_call, в Multicall3 не уходит.

## 5. Как сделано (решение 325)

- `node/prefetch.rs` — книга ответов на задаче (task_local, как `req_trail.rs`); `call::view`, `multicall::aggregate`
  (значит view_many / try_many), `solidly_fee_read.rs` / `solidly_gate_read.rs` сначала смотрят в неё. Ответ есть (тот
  же контракт, данные, блок) — без узла; нет — к узлу, как раньше. Откат и сбой в книгу не кладутся.
- `pools/prefetch_read.rs` — перечитывание: вызовы прошлого раза (`prefetch_memo.rs`) + что вид знает заранее
  (`prefetch_plan.rs` first) → одно обращение (частями по dex.load_batch); по ответам — зависящее (`more`) → следующее;
  затем ПРЕЖНИЙ `load::refresh` с книгой. Итог тот же по построению: тот же код чтения и разбор на том же блоке.
- `prefetch_plan.rs` (V2/V3/CL — функциями пачки `load_batch_*`; Elastic, DODO), `prefetch_ticks.rs` (Slipstream,
  Algebra V1.9), `prefetch_integral.rs` (Integral: дерево, `walk_plan` — тот же порядок и остановка, что у `walk`).
- `integral_fee.rs` — «только комиссия»; правило DYNAMIC_FEE — одна функция `dynamic` у полного чтения и здесь.
- `reread_bins.rs` + `load_bin::many` — бины Infinity Bin события одной пачкой.

## 6. Не входит в t152 (зафиксировано, не трогал)

- Вся сессия пачки ждёт перечитывания: одна задача на пачку — цикл `tokio::select!` в `pools/session.rs:104-160`
  ждёт `on_log(...).await` (`:114`), пока идёт перечитывание пула, логи остальных пулов пачки (в т. ч. V3) лежат в
  канале. Сколько ждало к концу перечитывания — поле `queued` записи dex.pool_refresh (`refresh_event.rs:98`,
  `refresh_timing.rs:47,112`); цифр queued в фактах задачи нет — смотреть в журнале живого бота. Отдельная задача.
- Проверка маршрута перед сделкой (quote/verify.rs, cycle_legs.rs), off_map для Bin/LB, per_block, первая загрузка.
- `check/quoter_block.rs:76` и `reload.rs:118` тоже зовут `load::refresh` — не перечитывание по событию, не тронуты.
