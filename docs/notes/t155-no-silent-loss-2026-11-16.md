# t155 — ни одно сообщение и ни один кандидат не теряются молча (решения 333, 334)

Дата: 16.11.2026. Ветка `t155/no-silent-loss` от `rel/dex-all` 8fe83aa1. Локально не собиралось (по правилу задачи).

Требование Тиграна 16.11: решение глобальное — все каналы, все площадки, все виды сообщений. Решения 321 и 324
закрыли только события пулов DEX и кадры стаканов бирж (+ Invalidated бирж); остальное оставалось.

## Как искалось (факт кода)

По всему `src/`: `try_send`, `try_reserve`, `TrySendError`, `.send(` в каналы (`mpsc`, `broadcast`, `watch`,
`unbounded`), `blocking_send`, `RecvError::Lagged` (отставание подписчика broadcast = пропущенные сообщения),
очереди с пределом (`queue_max`, «снять из очереди»). Отправки в сокеты WebSocket площадок (`sink.send`) — не каналы
бота, в таблицу не входят (ошибка отправки там — обрыв соединения, обрабатывается переподключением).

Канал событий ядра один на все площадки: `core/engine.rs` `mpsc::channel(deps.buffer)` (ёмкость `--live-buffer`),
клон раздаётся каждой площадке через `Venue::subscribe_market`.

## Что было сделано

- **Решение 333** — `gateway/core_out_must.rs` `MustOut::send`: все сообщения адаптеров ядру, что слались
  `let _ = out.try_send`, теперь идут через одну очередь «ждут места» на канал ядра (`Feed::must`, `gateway/core_out.rs`):
  - род **Money** (ядро по ним решает о деньгах): `WalletQueue`, `ExitGap` — настройка `venue.money_events_keep`;
  - род **Info**: `Notice`, `SendChannel`, `CoinCheck` — настройка `venue.info_events_keep`;
  - «да» — при полном канале ждёт места, уходит по порядку (FIFO); не «да» / не задана — как раньше потеряно, но с
    записью журнала `venue.core_events_lost` (счёт).
- **Решение 334** — кандидаты и прочие потери:
  - `core/cand_wait.rs`: кандидат при полной очереди расчётов или достигнутом `profit.calc_concurrency` — ждёт места
    не дольше `profit.calc_wait_ms`; не задана — как раньше выброшен, но со счётом и записью `core.candidate_lost`;
  - `gateway/lost_log.rs` — общий счёт потерь и запись журнала: на 1-ю, 2-ю, 4-ю… потерю или не чаще
    `log.events_lost_interval_s`;
  - теневые расчёты (`core.busy_calc`), отставание ожидания тика застрявшей сделки (`core.coin_ticks_lost`,
    `stuck.tick_lag_wake`), отставание дубля Telegram от событий ядра (`notify.core_events_lost`).

### Почему для Money/Info — очередь по порядку, а не keep_last (решение 324)

keep_last хранит по ключу ПОСЛЕДНЕЕ значение — годится для полного состояния (стакан, пул). Эти сообщения — события:
- `WalletQueue`: ядро шлёт уведомление по ПРЕДЫДУЩЕМУ состоянию (`core/queue_notify.rs`: стала «остановлена» →
  `send.queue_halted`; была «остановлена», стала «свободна» → `send.queue_released`). Слияние «остановлена → свободна»
  в одно «свободна» потеряло бы уведомление об остановке;
- `ExitGap` — замер одной сделки; `Notice` — отдельный текст; ключа «последнее заменяет прежнее» у них нет.

Отдельная задача с `send().await` на каждое сообщение порядок не держит (задачи tokio стартуют в любом порядке).
Поэтому — одна FIFO на канал и один доставщик (`reserve().await`): ждущий доставщик стоит в честной очереди семафора
канала tokio вместе с доставщиком набора keep_last — никто не голодает. Пока очередь не пуста, новое сообщение встаёт
в конец (не обгоняет). Сообщения редкие (на транзакцию, сделку, уведомление) — очередь на горячем пути не растёт.

## Таблица всех мест отправки в каналы и очереди

Обозначения: **не теряется** — отправка с ожиданием места / безразмерный канал / последнее значение по замыслу;
**теряется** — что было и что сделано.

### Адаптеры → ядро (канал событий ядра, `MarketEvent`)

| Место | Что | При полном канале было | Чем грозит потеря | Сделано |
| --- | --- | --- | --- | --- |
| venues/bsc_dex/send/settle.rs:198 (emit) → send/stats.rs emit | WalletQueue «ждёт итога / свободна / остановлена» | `let _ = try_send` — молча потеряно | «остановлена» потеряна → ядро пустит сделку биржа → DEX в остановленный кошелёк (core/wallet_queue_gate.rs); нет send.queue_halted | Money: ждёт места (venue.money_events_keep) или потеря со счётом venue.core_events_lost |
| venues/bsc_dex/wallet/deposit.rs:114 → stats.emit | WalletQueue «снята» (ручная проверка) | молча потеряно | направление биржа → DEX остаётся закрытым; нет send.queue_released | Money (как выше) |
| venues/bsc_dex/resubscribe_wallets.rs:44 | WalletQueue «остановлена» после запуска (из SQLite) | молча потеряно | как «остановлена» выше | Money (MustOut напрямую) |
| venues/bsc_dex/exec/exit_gap.rs:32 → stats.emit | ExitGap (замер выхода) | молча потеряно | монета не закрывается для входа (core/exit_gap.rs, guard.exit_gap_pct) | Money |
| venues/bsc_dex/exec/bundle_notify.rs:29; verify_gate.rs:173; swap_send.rs:152; trade_direct.rs:66; trade_smart.rs:68; executor/deploy.rs:169; executor/v1_notice.rs:48 → stats.emit | Notice исполнения DEX | молча потеряно | уведомление Telegram не приходит | Info: ждёт места (venue.info_events_keep) или потеря со счётом |
| venues/bsc_dex/send/stats.rs:73 (update) | SendChannel (счётчики канала отправки) | молча потеряно | экран каналов показывает старые счётчики до следующей транзакции | Info |
| venues/bsc_dex/check/tax_cycle.rs:68 | CoinCheck «не проверена» в начале цикла | молча потеряно | ядро не видит «ждёт своих пулов» (торговли нет и без этого события — монеты без итога в ядре нет) | Info |
| venues/bsc_dex/check/tax_pass.rs:203 | Notice dex.check_not_configured | молча потеряно | уведомление не приходит (журнал пишется) | Info |
| venues/bingx/rate_pause.rs:188 | Notice BingX 418 | потеряно; журнал писал «нет канала» (неверная причина) | уведомление не приходит | Info; пометка журнала называет точный исход (ждёт / потеряно / закрыт / нет канала) |
| venues/gate/withdraw_notice.rs:60 | Notice exec.withdraw_fail_pending | потеряно; журнал писал «нет канала» | уведомление не приходит | Info; точная пометка исхода |
| 14 бирж: */ws_conn.rs, xt/ws_book.rs, bitget/mod.rs (снимки REST) → gateway/core_out_book.rs | Book (стакан) | решение 324: venue.book_events_keep = да — последний кадр ждёт; иначе потеря со счётом адаптера | ядро пересчитает пару по следующему кадру | Не менялось (324). Запись журнала — только при заданном log.pool_events_lost_interval_s, иначе tracing (см. «Что осталось») |
| 14 бирж: */ws.rs invalidate → core_out_book.rs invalidated | Invalidated биржи | не теряется (324, срочное) | — | — |
| venues/bsc_dex/pools/emit_pool.rs | Pool | решение 321: dex.pool_events_keep | — | Не менялось (321). Журнал — log.pool_events_lost_interval_s |
| venues/bsc_dex/pools/invalidated.rs:33 | Invalidated пулов | try_send, полон — send().await в задаче: не теряется | — | — |
| venues/bsc_dex/node/receipts_notice.rs:30, head_stall_send.rs:35 | Notice узла | try_send, полон — send().await в задаче: не теряется | — | — |
| venues/bsc_dex/pools/graph_alarm.rs:96, loading.rs:137, check/quoter_pause.rs:158, check/tax_cycle.rs:209 (CheckPass), check/cycle.rs:161 (PoolCheck), check/tax_pass.rs:211, check/taxes_util.rs:180, check/service_tax.rs:162 (CoinCheck) | Notice, PoolsLoading, CheckPass, PoolCheck, CoinCheck | `send().await` — ждёт: не теряется | — | — |
| venues/bsc_dex/wallet/events.rs:162, 175; все биржи */user_stream*.rs | Balance | `send().await` — не теряется | — | — |
| все биржи */pairs_watch.rs, */market.rs | SpotPairs | `send().await` — не теряется | — | — |
| venues/kyberswap/market.rs:119, 130, 147 | Pool, Invalidated, CoinCheck | `send().await` — не теряется | — | — |

### Ядро — внутренние очереди

| Место | Что | Было | Чем грозит | Сделано |
| --- | --- | --- | --- | --- |
| core/candidates.rs:83 (enqueue) | кандидат в очередь расчётов (cand_tx) | полна — молча снят | тихая монета теряет кандидата до следующего тика | profit.calc_wait_ms: ждёт места; иначе потеря со счётом core.candidate_lost |
| core/candidates.rs:135 (run) | предел profit.calc_concurrency | достигнут — молча снят | то же | то же |
| core/busy_calc.rs:188 | теневой расчёт (только наблюдение) | полна — молча снят | неполная сводка замера | счёт и журнал core.busy_calc |
| core/window_run.rs:132 | сообщение задаче окон | счёт + журнал на 1-й, 2-й, 4-й… | окно неполное | уже видно — не менялось |
| core/episode_run.rs:145 | сообщение задаче эпизодов | счёт + журнал на 1-й, 2-й, 4-й… | эпизод неполный | уже видно — не менялось |
| gateway/tap.rs:88 | кадр записи окна | счётчик lost → сводка окна (core/window_close.rs) | запись окна неполная | уже видно — не менялось |
| core/uncapped_queue.rs:115, 127 | расчёт «без потолка» | счёт skipped_full на экране «Упущено» | — | уже видно — не менялось |
| core/tick.rs:117, 131 (slow_tx) | медленная обработка событий | безразмерный канал — не теряется | — | — |
| core/tick_sched.rs:33, 39 → coin_ticks (broadcast) → core/stuck.rs:121 next_tick | тик монеты застрявшей сделки | Lagged — молча ждали следующего тика | «Ожидание» пропускает проверку «стало выгодно» | счёт и журнал core.coin_ticks_lost; stuck.tick_lag_wake = да — проверка сразу |
| core/view.rs:77 → api/ws.rs | живой поток интерфейса (CoreEvent) | Lagged — интерфейсу сообщение об отставании (решение 109), перечитывает по REST | — | уже видно — не менялось |
| core/view.rs:77 → notify/mirror.rs:80 | события ядра для дубля Telegram | Lagged — молча | «Выполнено: итог» команды может не прийти; записи сверяются каждый круг | счёт и журнал notify.core_events_lost |

### Сервисы

| Место | Что | Было | Сделано |
| --- | --- | --- | --- |
| settings/store.rs:287 → settings/live.rs Watch::next / try_next | изменения настроек подписчикам | Lagged — tracing warn, пробуждение по пропущенному изменению теряется (значения компоненты читают из хранилища при использовании) | не менялось — см. «Что осталось» |
| gateway/setting_cell.rs:75, core/book_marks_on.rs:71, venues/bsc_dex/pools/revive.rs:53 | изменения настроек | Lagged — перечитывают всё: не теряется | — |
| storage/journal.rs:110, journal_repeat.rs:62, 113 → api/ws.rs | живой журнал интерфейса | Lagged — интерфейсу сообщение об отставании | — |
| storage/raw_log.rs:87 | сырые сообщения на диск | очередь > raw.queue_max — потеря, tracing на 1-й, 2-й, 4-й… | не менялось — см. «Что осталось» |
| storage/episode_files.rs:59–75 | файлы эпизодов и окон | безразмерный канал — не теряется | — |
| report/mod.rs:166, report/cap_send.rs:153 | файлы отчётов | blocking_send — ждёт | — |
| venues/bsc_dex/node/ws.rs, receipts.rs:164, receipts_handle.rs, pools/{group,attach,index,fee_watch,created,factory_scan}.rs, wallet/events.rs:102, directory/refresh.rs:149, gateway/feed_reg.rs | внутренние каналы адаптера DEX / шлюза | безразмерные — не теряются | — |
| venues/bsc_dex/node/heads.rs, receipts.rs (watch) | номер блока, последняя квитанция | watch — последнее значение по замыслу | — |
| venues/xt/ws_conn.rs:86 → xt/snapshots.rs:79 | снимки REST XT | `send().await` — ждёт | — |

## Что будет при шквале

- **Money / Info при «да»:** очередь растёт на число сообщений, пришедших, пока канал полон; эти сообщения редкие
  (на транзакцию, сделку, уведомление, монету в начале цикла налогов — сотни, не тысячи в секунду). Доставщик отдаёт
  их по порядку вперемешку (честно) с наборами стаканов и пулов. При «нет» — как раньше, но потеря видна в журнале.
- **Кандидаты при profit.calc_wait_ms:** ждущих не больше числа направлений (направление стоит в очереди один раз —
  `Inner.queued`; новый тик того же направления с ним сливается). Каждый ждёт не дольше срока, затем потеря со счётом;
  следующий тик ставит снова. Ждущие стартуют первыми, теневые — только когда ждущих нет. Срок profit.calc_timeout_ms
  — от старта расчёта, ожидание в него не входит (видно в замере «очередь до старта», решение 129). Расчёт берёт рынок
  на момент старта.
- **Журнал:** не задан log.events_lost_interval_s — строки на 1-й, 2-й, 4-й, 8-й… потере (на миллион потерь — около
  20 строк на место); задан — не чаще раза в интервал на место.

## Что осталось (не исправлено в t155 — честно)

1. Стаканы бирж (324) и события пулов (321) при незаданной настройке «откладывать» и незаданном
   log.pool_events_lost_interval_s: потеря — только счёт адаптера и tracing (журнал процесса), не журнал интерфейса.
   FIRST_RUN уже советует log.pool_events_lost_interval_s = 60. Не менялось, чтобы не трогать горячий путь 324 (Б2).
2. Подписчики изменений настроек (settings/live.rs Watch): отставание — tracing warn, пробуждение по пропущенному
   изменению может не случиться (значения при этом читаются из хранилища при каждом использовании).
3. storage/raw_log.rs: потеря сырых сообщений на диск — tracing на 1-й, 2-й, 4-й…, не журнал интерфейса.
