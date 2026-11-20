# settings/catalog — каталог настроек (стык)

Зеркало `docs/SETTINGS.md`. Значений по умолчанию нет. Имена уникальны (проверяет тест в `mod.rs`).

| Файл | Что внутри |
| --- | --- |
| `mod.rs` | Короткая запись `d()`, наборы областей, `all()` и `find()` по всем частям |
| `base.rs` | Деньги и риск, расчёт, сигнал, исполнение, переводы, площадки (биржи), уведомления, безопасность, система |
| `trade.rs` | Налоги и крючки, возврат USDT, застрявшие монеты, ручной чёрный список (решения 30, 32, 34, 36, 37); решение 293 (ревью) — stuck.reject_route_pause |
| `sizing.rs` | Решения 343–345 (задача t159, ревью t159): size.scan_above_pct, size.pool_breaks_max, size.soft_deadline_pct — параметры-числа подбора объёма без потери прибыльных объёмов (все необязательные); порог % (342) и участки по стакану B (343) — в логике |
| `cpu.rs` | Решения 336–338 (задача t157): size.parallel_points, size.bisect_probes, size.early_exit_margin_usd, cpu.threads_max, cpu.reserve_deal, cpu.reserve_calc, cpu.helper_threads, dex.quote_threads, dex.graph_threads — счёт процессора (все необязательные) |
| `dex.rs` | Узел BSC, режим обмена, пулы, умный режим, адреса контрактов вида пула (решения 29, 31, 38) |
| `dex_fit.rs` | Решение 294: dex.intermediate_partial_fill — путь через промежуточную отдаёт помещающуюся часть (отдельный файл: dex.rs — зона другого разработчика); решение 300: dex.graph_exec_fallback — лучший исполнимый маршрут графа |
| `dex_graph.rs` | Решение 308: dex.graph_background, dex.graph_max_lag_ms, dex.graph_hang_ms, dex.graph_retry_pause_s, dex.graph_stage_b_max_ms, dex.graph_verify_s, dex.graph_alert_min_s — граф маршрутов без ожидания (фоновый владелец, предохранитель, сторож, самопроверка); решение 310 — dex.check_due_read_only (достоверность пулов циклами проверок без записи в реестр) |
| `dex_fit.rs` | Решение 294: dex.intermediate_partial_fill — путь через промежуточную отдаёт помещающуюся часть (отдельный файл: dex.rs — зона другого разработчика); решение 300: dex.graph_exec_fallback — лучший исполнимый маршрут графа; решение 311 А: dex.quote_prep_once — выключатель одной подготовки глубины на котировку графа (не задана — включено) |
| `dex_graph.rs` | Решение 308: dex.graph_background, dex.graph_max_lag_ms, dex.graph_hang_ms, dex.graph_retry_pause_s, dex.graph_stage_b_max_ms, dex.graph_verify_s, dex.graph_alert_min_s — граф маршрутов без ожидания (фоновый владелец, предохранитель, сторож, самопроверка) |
| `dex_lb.rs` | Решение 446: dex.lb_implementations, dex.lb_hooks_allowed, dex.lb_hooks_code_hashes (ревью) — Liquidity Book V2.2 (реализации пула, сверенные с исходником; допущенные крючки обмена и хеши их кода) |
| `dex_event.rs` | Задача t144: решение 313 — log.pool_refresh_timing, log.pool_events_lost_interval_s (замер перечитывания пула по событию, потери событий пулов при полном канале ядра); решение 314 — dex.event_refresh_urgency (класс очереди узла для перечитываний по событию); решение 315 — dex.event_refresh_one_call (пул V3 по событию одним обращением к узлу); решение 325 — dex.event_refresh_batch, dex.event_refresh_fee_only (область «DEX»: перечитывание по событию любого вида с ответами, прочитанными заранее; у Algebra Integral — только комиссия); решение 332 — dex.bin_reread_left_pct [DEX] (перечитывание пула из бинов по выходу цены за прочитанную полосу); ревью 332 — log.bin_reread_interval_s (частота записей журнала этой проверки); решение 339 — log.pool_break_interval_s (журнал обрывов пачки) |
| `write_path.rs` | Решения 340, 341, 353 (задача t158): notify.queue_max, journal.queue_max, journal.batch_max, ledger.synchronous — путь сделки без ожиданий |
| `book_events.rs` | Решение 324 (задача t151): venue.book_events_keep — кадр стакана биржи при полном канале ядра не теряется (последний кадр пары ждёт в общем наборе канала ядра) |
| `lost_events.rs` | Решения 333/334 (задача t155): profit.calc_wait_ms, venue.info_events_max, venue.money_events_alert_ms (ревью 333; venue.money_events_keep удалена — денежные не теряются никогда; venue.info_events_keep удалена — решение 362), log.events_lost_interval_s — сообщения ядру и кандидаты не теряются молча |
| `sieve.rs` | Решения 372–375: sieve.audit_share — доля контроля отсева по границе на живом боте |
| `index.rs` | Справочник пулов от индексатора: адрес API, сеть, ключ, темп, страницы, частота, порог, подпись (решение 58) |
| `send.rs` | Отправка транзакций и каналы отправки (решения 33, 41–45) |
| `signals.rs` | Сигналы и эпизоды: сухой режим, предел поиска максимального объёма, шаблоны ссылок на пару и пул, эпизоды (решение 74) |
| `uncapped.rs` | Упущено из-за потолка (решение 141): uncapped.enabled, uncapped.max_concurrent, uncapped.min_interval_ms, uncapped.queue_max, uncapped.max_evals, uncapped.ladder_usdt, uncapped.episode_gap_ms, uncapped.keep_days; решение 245 — uncapped.maxwin_gap_ms, uncapped.deal_time_min_n |
| `window.rs` | Окна (решение 125): window.enabled, window.min_hold_ms, window.queue_max, window.max_mb, window.keep_days, window.zstd_level, window.cleanup_s, window.dex_price, window.dex_price_min_ms (решение 241), window.dex_pool_state (решение 255), window.busy_calc, window.busy_calc_min_ms (решение 267), venue.trades_interval, venue.window_depth_interval, venue.window_depth_limit [mexc] |
| `bingx.rs` | Только BingX (решение 293): venue.ignore_deprecated_qty — не соблюдать устаревшие minQty / maxQty справочника пар |
| `gate.rs` | Только Gate (решение 135): venue.book_interval, venue.window_diff_interval, venue.rate_limit_pause_ms, venue.order_history_limit |
| `dex_fail.rs` | Задача t153, решение 339: dex.pool_fail_pause_ms, dex.pool_fail_pause_max_ms — ошибка чтения одного пула не рвёт сессию пачки, повтор пула с растущей паузой |
| `dex_par.rs` | Задача t153: решение 331 — verify.read_parallel_max (пулы маршрута перед сделкой и ног цикла DEX ↔ DEX одновременно); решение 330 — dex.event_lanes_max, dex.block_read_parallel_max (сессия пачки) |
| `dex_ekubo.rs` | Решение 447: Ekubo — dex.ekubo_core, dex.ekubo_key_index, dex.ekubo_mev_capture (необязательная), dex.ekubo_bitmap_words |
