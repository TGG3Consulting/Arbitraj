# t175 — продажа ↔ застрявшая монета без рекурсии (решение 389), 17.11.2026

Ветка t175/sell-stuck-no-recursion от f9634504. Локально не собиралось (cargo не запускался) — сборка и тесты в GitHub.

## Что случилось (факты журнала сервера, переданы координатором)

Сборка f9634504 упала через 37 минут: `thread 'tokio-rt-worker' has overflowed its stack`. Стек-трейса нет — core
dump не сохранился. Сделка 20261117143812842GENIUS (bsc_dex → binance):

- 14:38:13 куплено на bsc_dex; 14:38:14 перевод на binance 694,214 GENIUS; 14:38:40 зачислено (27,4 с);
- 14:38:41 core.stuck: «продавать невыгодно», режим relay;
- 14:38:41–14:38:44 четыре ордера продажи на binance, все EXPIRED (UNFILLED_IOC_QUANTITY_EXPIRED, исполнено 0 из
  694,2), каждый раз op_error OrderNotFilled, повторы 1/3, 2/3, 3/3;
- 14:38:44.135 core.sell_unsent_here: «повторы исчерпаны (stuck.unsent_retries_max = 3)», продажа здесь по порогу
  stuck.relay_max_loss_pct (решение 229);
- дальше записей по сделке нет; последняя запись журнала 14:38:44.336 (отказы кандидатов), процесс упал.
- За 10 минут до падения: core.stuck 12, core.sell_unsent_retry 5.

## Что найдено (факт кода f9634504)

Цепочка «продажа ↔ застрявшая монета» — async-рекурсия. Звенья (перепроверены по коду, строки — f9634504):

- sell.rs:72-74 sell_or_stuck → :85 sell_or_stuck_guarded → :93/:96 `Box::pin(stuck::handle)` (невыгодно / нет
  котировки);
- stuck.rs:43-49 handle (сверка остатка remain_fact) → :52 handle_as_is → :92-94 wait / loss_limit / relay::run;
- relay.rs:105-201 → relay_here::sell_here (перегонов не осталось / площадка или кошелёк на паузе / вывод отклонён /
  перегон не выгоднее), :258 `Box::pin(sell::sell_after_relay)` → sell_or_stuck_guarded, :281 transfer_failed →
  sell_here;
- relay_here.rs:84-106 sell_here → :102 stuck::sold_by_market; stuck.rs:188-199 sold_by_market → sell::market_guarded;
  stuck.rs:129 wait → market_guarded; stuck.rs:141, :174 → sold_by_market;
- sell.rs:122-141 market_guarded → patience_scope::with → sell (sell.rs:172);
- sell.rs:302-326 итог продажи — ошибка: `Retry::Go` → `Box::pin(risk_count::auto(sell_unsent::retry))` (:316-317),
  `Retry::Here` → `Box::pin(risk_count::auto(sell_unsent::here))` (:322-325), кнопка → manual_sell::retry (:306);
  ещё :245 manual_sell::remainder, :253 `auto(stuck::handle)` (остаток), :273 manual_sell::gas, :281
  `auto(stuck::handle)` (газ BNB); sell_range_run.rs:75 → stuck() :265-274 `Box::pin(stuck::handle)` (решение 268);
- sell_unsent.rs:131-168 retry → :167 `Box::pin(stuck::handle)`; :174-197 here → :192 remain_fact::reconcile → :196
  `Box::pin(relay_here::sell_here)`;
- manual_sell.rs:30-53 retry / :56-63 remainder → :74-85 again → :84 `Box::pin(sell::market)` → market_guarded.

Box::pin уносит будущее следующего уровня в кучу, но опрашивается оно изнутри poll предыдущего: кадры опроса всех
уровней лежат на стеке потока одновременно. При OrderNotFilled на бирже пауза 0 (sell_unsent.rs:73, not_filled_cex) —
уровни вкладываются сразу.

**Поиск всех таких мест.** Граф вызовов функций ядра (src/core/*.rs, по именам `super::модуль::функция(` и вызовам
внутри файла) — поиск сильно связанных компонент. Найдена ровно одна рекурсия с ростом стека — 20 функций выше
(sell::{sell, market, market_guarded, sell_or_stuck_guarded, sell_after_relay}, sell_unsent::{retry, here},
stuck::{handle, handle_as_is, wait, loss_limit, sold_by_market}, relay::run, relay_here::{sell_here, transfer_failed},
manual_sell::{retry, remainder, again}, sell_range_run::{run, stuck}). Остальные найденные циклы — не рост стека:
uncapped_queue (later / run — через tokio::spawn, каждая итерация — своя задача), signal_ripen::arm (повтор — внутри
tokio::spawn), rebuild::run (цикл loop), прочие — совпадение имён функций и методов, не рекурсия. Возврат USDT
(usdt_return_job.rs, usdt_return_retry.rs — tokio::spawn; usdt_relay_retry.rs — цикл), executor.rs (цикл loop
пересчёта), precheck_dex_sell.rs (once → replan → once, глубина 2) рекурсии с ростом не имеют.

**Глубина на пути аварии (до исправления).** executor.rs:600 → sell_or_stuck → sell_or_stuck_guarded → stuck::handle →
handle_as_is → relay::run → sell_here → sold_by_market → market_guarded → with → sell (ордер 1) → auto → retry →
handle → handle_as_is → relay::run → sell_here → sold_by_market → market_guarded → with → sell (ордер 2) → … (ордер 3)
→ … (ордер 4) → auto → here → remain_fact / sell_here → котировка (REST) → sold_by_market → market_guarded → sell
(ордер 5 — в журнале его нет). Один уровень продажи — 10 вложенных async-функций (market_guarded, with, sell, auto,
retry, handle, handle_as_is, relay::run, sell_here, sold_by_market; при заданной exec.rate_limit_wait_max_ms ещё
patience::scope) плюс коробки. В точке падения — 4 полных уровня (ордера 1–4: кадр каждой sell жив, пока идёт её
продолжение) + вход (6 функций) + уровень «здесь» и путь запроса адаптера: ≈ 50 вложенных кадров опроса async-функций
сверх пути задачи сделки. После исправления на любом повторе — вход (sell_or_stuck_guarded → hop → цикл) + один
уровень продажи, от числа повторов не зависит.

Замеры стека, которые есть (docs/ROADMAP.md, «Техдолг», замер 14.11): debug — один уровень авто-продажи с отказом
ордера ≈ 1,3 МиБ (tests/core_market_sell_268.rs:301-306); release — пути решения 268 целиком, вместе с запуском ядра
на том же потоке: entry_unprofitable_goes_to_mode_then_mode_sells_market (два уровня продажи: sell → режим → sell)
≈ 730 КиБ, mode_sale_within_threshold_goes_market_once (один уровень) ≈ 710 КиБ; стек потока tokio по умолчанию —
2 МиБ (в коде размер не задаётся). Это разные пути, прямого замера «сколько добавляет один уровень» в release нет.
Если считать разницу 730 − 710 оценкой уровня, пять уровней пути аварии в 2 МиБ укладывались бы — то есть по
имеющимся замерам НЕ доказано, что одной этой рекурсии хватало для переполнения в release. Рекурсия — единственный
найденный путь, где стек растёт по ходу работы, и падение пришлось ровно на его самую глубокую точку; но причина
аварии остаётся гипотезой.

## Что сделано

- core/sell_loop.rs (новый) — батут `hop`: первый вызов в задаче становится циклом (`drive`, ячейка task_local), каждое
  следующее продолжение, пришедшее изнутри текущего, откладывается в ячейку и исполняется циклом после возврата — на
  одном уровне стека. Признаки задачи, видимые в месте откладывания (risk_count «ручное», gateway/patience DEADLINE и
  EXEC), запоминаются и ставятся заново вокруг продолжения (`carry`). Ячейка занята — продолжение исполняется сразу,
  как раньше (по коду такого не бывает; страховка «ничего не терять»).
- core/sell_next.rs (новый) — продолжения в коробке (не-async функции): stuck (с risk_count::auto или без — как было
  в каждом месте), unsent_retry, unsent_here, button_retry, button_remainder, button_gas. Ядро — клоном, ошибка и
  причина — копией.
- sell.rs: все 9 мест продолжений (:93, :96, :245, :253, :273, :281, :306, :316-317, :322-325) — `sell_loop::hop`.
  sell_range_run.rs stuck() — тоже через hop. После удаления этих рёбер граф ядра без циклов (проверено тем же
  поиском).
- risk_count.rs, gateway/patience.rs — snapshot / restore признаков.

**Почему поведение то же.** Каждый кадр между циклом и местом откладывания — хвостовой вызов: после него по коду
ничего не делается (sell_unsent::retry/here, stuck::handle/handle_as_is/wait/loss_limit/sold_by_market, relay::run,
relay_here::sell_here/transfer_failed, manual_sell::retry/remainder/again, market_guarded, sell_or_stuck_guarded,
sell_after_relay, sell — проверено по каждому; sell_range_run::run после stuck возвращает true, и sell сразу выходит).
Значит следующее продолжение начинается после тех же шагов, что и раньше, — те же ордера, число повторов
(stuck.unsent_retries_max), паузы (stuck.reject_retry_pause_ms — внутри sell_unsent::retry, без изменений), записи
журнала, строки хронологии, уведомления; правило «ордер отправлен и не исполнился — отменяем» (покупки) не затронуто.
Цикл продолжает в том же опросе (без уступки планировщику) — задержки нет. Отличие одно: локальные переменные
закончившегося уровня (например подписка на тики Ticks в stuck::wait) освобождаются раньше — наблюдаемых
последствий по коду нет.

**Почему не tokio::spawn.** Новая задача не видит признаков задачи сделки (urgency «Trade», cpu_budget ярус «сделка»,
risk_count, patience и др. — их больше десятка в gateway/ и core/) и не отменяется вместе с ней — это изменило бы
поведение. Цикл в той же задаче сохраняет всё, что выше цикла, автоматически.

## Что доказано тестом и что нет

- tests/core_sell_stuck_chain_t175.rs (стенд XT как tests/core_market_sell_268.rs; ответы на ордер — по документации
  XT, не записаны): путь аварии — sell_or_stuck (невыгодно) → режим relay (stuck.max_relays = 0 → продажа здесь,
  порог 5 %) → ордер не исполнился (OrderNotFilled) → повторы → продажа здесь → алерт. (а) При
  stuck.unsent_retries_max = 3: 5 ордеров, порядок строк хронологии (вход в режим, k/3 и снова вход ×3, «повторы
  исчерпаны … продажа здесь по порогу», алерт) и журнала (core.stuck, [core.sell_unsent_retry, core.stuck] ×3,
  core.sell_unsent_here, core.sell_unsent_exhausted). (б) То же при 200 повторах в потоке с тем же стеком 16 МиБ.
- tests/core_sell_loop_t175.rs (чистая логика): 100 000 звеньев через hop в потоке со стеком 1 МиБ — порядок шагов как
  у вложенных вызовов; продолжение видит те же «ручное» и терпение, что видело бы вложенным.
- **Не доказано прогоном:** что до исправления тест (б) переполнил бы стек — старый код этим тестом не запускался
  (cargo локально не запускается; падение переполнения роняет весь процесс тестов). Ожидание по коду: 200 вложенных
  уровней по 10+ кадров. **Не доказано**, что авария вызвана именно этой рекурсией: трассы нет. Биржа стенда — XT, а не
  binance (путь ядра от площадки не зависит: OrderNotFilled ставит core/trade_ops.rs по итогу NotFilled любого
  адаптера). Записанного ответа binance EXPIRED в tests/data нет.

## Что подтвердило бы причину

Стек-трейс уже не получить. Косвенно (только чтение, сервер): повторится ли падение на сборке с решением 389 при той же
цепочке (core.sell_unsent_here по сделке биржи с OrderNotFilled). Если падение повторится — причина в другом месте,
нужна трасса (включить сохранение core dump: `ulimit -c unlimited` / systemd LimitCORE на сервере — решение владельца).

## Что по-прежнему может переполнить стек (по коду)

- Один уровень пути сделки с запросом адаптера (release ≈ 0,7 МиБ на путях решения 268 вместе с запуском ядра) — рост
  с числом повторов убран, но если причина аварии другая (не рекурсия), этот уровень плюс что-то ещё на том же потоке
  остаются; без трассы не видно. Нужен прямой замер в release пути аварии до и после решения 389 (тем же способом, что
  в docs/ROADMAP.md: перебором размера стека потока теста tests/core_sell_stuck_chain_t175.rs) — docs/TASKS.md.
- Ячейка занята — продолжение вложенно (как раньше); по коду не бывает.
