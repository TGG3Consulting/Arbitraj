# t164 — ревью слияния «357 × 339»: правило достоверности пулов с отметками (Б1, Б2, Н1–Н5, Н7)

Ветка `t164/merge-fix` от `rel/dex-all` 906ba0d9 (объединённый релиз t152–t158, t160, t161). Отчёт ревью —
scratchpad review-merge-rel.md. Каждое замечание перепроверено по коду 906ba0d9: Б1, Б2, Н1–Н5, Н7 — верны.

## Правило (одно на все виды с отметками: LB V2.1, Infinity Bin, Algebra Integral)

`valid = true` пулу возвращают ТОЛЬКО:
1. **полное чтение пула** — любое: повтор (`reload::retry`/`load_many`), `load_one(_block)`, приём пачки (admit,
   wake), `depth_sync` resync/on_change, Refresh по событию, off_map, полное чтение на голове (`settle_close.rs`,
   `per_block_lb.rs` при отметке head). Все они кладут запись через `registry.rs put` с `valid = true` (`load::renewed`);
   `put` достоверной записи теперь снимает ВСЕ отметки пула (`registry.rs` put → `holds.forget`) — как отметки неудач 339;
2. **снятие Active / Follow логами ТОГО ЖЕ действия** (`settle.rs` release): Active — лог того же блока, у которого
   активный бин памяти = активному бину узла на конце блока (`settle_bins.rs`); Follow — лог той же транзакции того же
   блока (`settle.rs` step). Лог другого действия — `settle_lost.rs lost` → отметка долой БЕЗ возврата, пул на полное
   чтение (`listen_log.rs`: defer позиции, итог true → очередь повтора);
3. **thaw depth_sync** — только тем, кого он сам заморозил (а замораживает он только достоверные; события пачки в это
   время ждут в канале — пропусков нет).

Голова LB (`lb_head.rs`) достоверность не поднимает НИКОГДА (`renewed` — как была, `put` без head_ok): её сравнение
«равны» видит только активный бин и его резервы. Неудачная голова (`per_block_lb.rs`) — `settle_head.rs head_failed`:
недостоверен + «нужно полное чтение»; следующая голова (после паузы 339 — `per_block.rs` пул на паузе не читает) читает
его ЦЕЛИКОМ (`reload::load_one_block`).

## Таблица «состояние × событие → кто ставит / снимает valid»

Состояния: **А** достоверен; **Б** недостоверен по ошибке чтения (listen_log.rs ошибка, retry) — в очереди повтора;
**В** пауза 339 (пул недостоверен — пауза ставится только вместе с valid = false: listen_log.rs ошибка, block_failed,
settle_close, retry); **Г** Follow; **Д** Active; **Е** ждёт повтора после StaleData (решение 298); **Ж** спящий
(мельче dex.min_pool_liquidity_usd, слушается только ввод ликвидности); **З** заморожен depth_sync; **И** отметка head
(«нужно полное чтение»).

| Состояние \ событие | Лог того же действия | Лог другого действия / блока | Голова | Полное чтение | Ошибка чтения | Обрыв сессии | Пул удалён |
| --- | --- | --- | --- | --- | --- | --- | --- |
| А | применяется как раньше; Bins: Active-неравенство → Д (valid=false), SwapFee/CompositionFees → Г | — (отметки нет) | LB: только комиссия, valid как был; прочие виды «каждым блоком» — полное чтение | valid=true | valid=false → Б/В/Е | invalidate (valid=false) или dex.keep_pools_on_break (решение 93, как было) | retain |
| Б | waiting? нет — revive (339) → повтор; иначе событие применяется, valid не трогается | то же | LB: не поднимает | **единственный путь к valid=true** | остаётся Б | invalidate, приём читает целиком | retain, отметки долой |
| В | waiting: Bins/Updated без чтения, Refresh — defer; valid=false | то же | per_block.rs пул на паузе не читает | после паузы: повтор (Б/Е) или per_block_lb полное (И) → valid=true | пауза растёт | invalidate | retain |
| Г | лог той же tx → release (was_valid) | lost → forget, valid=false, defer, повтор (полное) | per_block_lb: pending — не трогает | put снимает отметку, valid=true | forget, valid=false → Б | forget_all + invalidate | retain → forget_all, market keep_only |
| Д | лог того же блока: бины копятся / читаются; активный = концу блока → release | lost → forget, полное чтение | settle_close: логам негде ждать → полное на голове (ошибка → Invalidated, 339, повтор) | put снимает | forget → Б | forget_all | retain |
| Е | waiting: без чтения; lost/step не работают при waiting | waiting | settle_close: только forget, читает retry | retry → valid=true | остаётся в очереди | invalidate | retain |
| Ж | лог ликвидности → wake: load_one (полное) | — | не читается | wake/депth_sync | missing | invalidate | retain |
| З | события ждут в канале пачки (сессия занята чтением) | — | — | thaw / полное | depth_sync fail → missing | — | — |
| И | событие: Bins (не pending) — читаются бины, valid не поднимается (release при head=true не возвращает) | lost — нет (need=None) | per_block_lb: **полное чтение** вместо головы; удачно → put снимает, valid=true; неудача → block_failed (пауза), отметка остаётся | put снимает | block_failed | invalidate снимает отметку; приём читает целиком | retain |

Почему не остаётся недостоверным навсегда: Б/Е — очередь повтора каждой головой (пауза 339 конечна,
dex.pool_fail_pause_max_ms); В — после паузы его читает тот, кто поставил (очередь повтора или per_block_lb по отметке
И); Г/Д — либо придёт лог своего действия, либо лог чужого (lost → повтор), либо голова при пустом пути
(`settle_close.rs`); И — каждая голова, пока не прочитается; Ж/З — как раньше. Брошенный повтором (dex.pool_load_retries
исчерпан без 339) — как раньше (решение 298: переподключение, Refresh, пересинхрон) — достоверным он не становится сам.

## По замечаниям

- **Б1 — исправлено.** `settle_head.rs` (head_ok удалён, needs_full), `lb_head.rs` put, `per_block_lb.rs` (отметка head →
  `load_one_block`), `registry.rs` put (отметки снимает только достоверная запись). Путь (а): пауза → после паузы
  per_block_lb читает пул целиком, бины, пропущенные на паузе, перечитаны. Путь (б): любое полное чтение снимает отметку
  head, а сама отметка больше ничего не возвращает. Тест — `tests/bsc_dex_settle_merge_357.rs`
  head_fail_then_pause_only_full_read_restores (настоящий лог Swap, применённый без перечитывания бинов: бин 8399652 в
  памяти старый, голова не поднимает, полное чтение на probe_time кладёт записанный резерв).
- **Б2 — исправлено.** `settle_lost.rs lost` + `listen_log.rs` (до применения лога), `settle.rs step` (Follow — та же
  tx: Hold.tx), `listen_log.rs invalidate` → forget_all. Тест hold_released_only_by_same_action.
- **Н1 — исправлено.** `listen_log.rs`: step (отметка Follow) — ДО `refresh_event::fee`; ветка Fee при отметке выходит
  после чтения комиссии.
- **Н2 — исправлено.** `settle_close.rs`: неудача — valid=false, `invalidated::send`, `pool_fail::note`,
  `pool_fail::requeue(base.is_some(), …)`.
- **Н3 — исправлено.** `settle_close.rs in_transit`: в полосе пула пусто и (канал пачки пуст ИЛИ сессия уже приняла лог
  глубокого пула более позднего блока — `session.rs seen`). Почему не закроет раньше логов пула: подписка «все события»
  — один упорядоченный по (блок, logIndex) поток; приняв лог блока > B, сессия приняла все логи блока B (они либо
  применены, либо в полосе пула — `lanes.queued(i)`). Оговорка — стык смены состава подписки (два потока): закрытие
  раньше даёт полное чтение на голове (лишняя работа, не деньги). Тест close_only_when_nothing_in_transit.
- **Н4 — исправлено.** `registry.rs retain` → forget_all, `market.rs` → keep_only. Тест (в hold_released_only_by_same_action).
- **Н5 — исправлено.** `reread_bins.rs` удалён (mod.rs, README, шапки event_flags.rs, load_bin.rs, listen_log.rs); текст
  dex.event_refresh_batch в `settings/catalog/dex_event.rs` и `docs/SETTINGS.md`: бины событий от настройки не зависят.
- **Н6 — по коду верно (ревью), без правок.** Порядок Fee изменён Н1.
- **Н7 — исправлено.** `settings/store.rs` set: значение перебито отметкой ядра (решение 355) — журнал «изменено» и
  рассылка не делаются, вызывающему — SetError::Invalid с причиной.
- **Н8** — дубли строк SETTINGS были до слияния; не трогались (правки — точечные).

## Тесты

`tests/bsc_dex_settle_merge_357.rs` (записи lb_v21_2026-11-15, узел на записи tests/lb_node); `tests/bsc_dex_settle_357.rs`
— тест головы переписан под правило (голова «равны» не поднимает и снятую ею достоверность). Без теста (нет записи):
Follow на настоящем SwapFee 1.2.10; вывод из неактивного бина с «равны» на голове; settle_close и сквозной путь сессии
(нужен Pools). Локально не собиралось.
