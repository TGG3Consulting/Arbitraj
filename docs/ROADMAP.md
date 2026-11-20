# ROADMAP — порядок разработки

> Решение Тиграна: сразу полный автомат, без этапа ручного подтверждения сделок.
> На каждом этапе интерфейс делается вместе с функционалом (см. CLAUDE.md → «Готовая фича»).

| Этап | Что делаем | Что видно в интерфейсе | Готово, когда |
| --- | --- | --- | --- |
| 0 | Каркас: структура папок, хранилище настроек, шлюз и контракт адаптера, веб-сервер, разблокировка секретов | Экраны «Разблокировка», «Настройки», «Журнал» | Настройка меняется в интерфейсе и сразу видна в системе |
| 1 | Адаптеры BingX и DEX в BSC: соединения, подписки, разбор, запись сырых сообщений | Экран «Площадки»: статусы, задержки, разрывы | Цены и пулы идут непрерывно, разрывы восстанавливаются сами |
| 2 | Справочник монет и направлений (шаг 1) | Экран «Монеты и направления» | Видно, какие монеты прошли фильтр и почему отсеяны остальные |
| 3 | Живые спреды и фильтр шума (шаги 2–3) | «Живой экран» | Спреды по обоим направлениям обновляются на каждый тик |
| 4 | Расчёт выгодности: затраты, ликвидность, объём (шаг 4) | Кандидаты и причины отказа на «Живом экране» | По каждому кандидату виден полный расчёт |
| 5 | Автоматическое исполнение, переводы, учёт, правила шлюза, стоп-условия (шаги 5–6) | «Сделки», «Балансы», «Правила шлюза», «Управление» | Бот сам проводит сделку от сигнала до сверки |
| 6 | Новые площадки: Gate, MEXC, другие DEX | Новые площадки на экране «Площадки» | Подключены по docs/ADAPTER_TEMPLATE.md без изменений ядра |

## Текущий приоритет (решение Тиграна, 2026-10-30)

Серия 80 (графовый роутер DEX) — порядок и что строго за ней:

1. **Движок графового роутера:** 80.6 (предторговая верификация маршрута точечным Multicall3, решение 83) → 80.8 (исполнение бандлом свопов через приватные релеи, решение 84).
2. **Сразу после серии 80 — полное покрытие ВСЕХ DEX на BSC (обязательно, до тестов):** Biswap (решение 82), KyberSwap Elastic (решение 81), экзотика (DODO, Wombat, Curve, WooFi, Maverick). Каждому семейству — математика его пулов; в граф попадают через единый движок, ядро не меняется.
3. **Второе направление DEX ↔ DEX (решение 85), до тестов:** на том же движке (граф решения 80 + атомарный бандл решения 84). Поиск замкнутых циклов в графе (старт = финиш = базовая монета), свой расчёт выгоды без затрат на перевод/вывод, слой стратегии/сигнала, переключатель направления в интерфейсе. Пять частей фичи. Делается после п.2 (нужен весь граф пулов).
4. **Параллельно (инфра):** свой сервер + синхронизация узла BSC.
5. **Тестирование — только после п.1–3, со ВСЕМИ дексами и ОБОИМИ направлениями сразу:** сухой режим на живых данных → затем живой.

Этап 6 таблицы (Gate, MEXC, другие СЕТИ) — после v1; к покрытию BSC-DEX не относится. «Другие DEX» в строке 6 означает DEX в других сетях, не на BSC: все DEX НА BSC входят в п.2 выше и делаются до тестов.

## Техдолг
- Решение 452 (T433-F2, доделка Balancer V3): обмен через буфер ERC4626 не строится (вопрос docs/TASKS.md «Решение 452»,
  п. 2); MEV Capture при сборе чаевых ArbOS — без котировки (п. 3); ReClamm 2025 при сдвиге — без котировки (п. 5);
  тест tests/arbitrum_balancer_v3.rs — #[ignore] до записи узла. `wc -l`: pools/created_log.rs 356 (было 349),
  pools/families.rs +2; pools/load_balancer_v3_kinds.rs 220 и load_balancer_v3.rs 201 — разбиты ревью (180 / 138, новые
  load_balancer_v3_reclamm.rs, load_balancer_v3_state.rs).
- Решение 441 (T433-F2, Balancer V3): событие PoolRegistered хранилища V3 — подключено решением 452; исполнитель v4 не пересобран (contracts/build.sh; out/v3/ — копия v3) и нет EVM-теста шага
  BALANCER_V3 (contracts/evmtest); тест tests/arbitrum_balancer_v3.rs — #[ignore] до записи узла; пулы с крючком обмена и
  виды Gyro / ReClamm / LBP не подключены (docs/TASKS.md, «Решение 441»). `wc -l`: math/balancer_v3.rs, load_balancer_v3.rs —
  в пределах 200; pools/families.rs (было 651) — давно сверх, +Proto::BalancerV3.
- Ревью 441 (п. 7): exec/executor/code.rs — 283 строки → 306 (+runtime_sha256_of, current_version), сверх ориентира 200
  (вынести версии v1–v3 в отдельный файл); live_proof.rs 156 → 198; пересборка v4 и её тесты — план в docs/TASKS.md
  («Решение 441», ревью).
- Решение 438 (ревью t222, п. H) и решение 439 (`wc -l`): сверх ориентира 200 — venues/bsc_dex/exec/cycle_exec.rs 217
  (было 212 до 439: +статистика условия пулов), send/ready.rs 238, exec/txs.rs 272, send/mod.rs 305 (было 302:
  +журнал «условие пулов при нескольких каналах»), send/track_l1.rs 208 (было 166: caught_up по времени блока и nonce
  на голове H), exec/pool_cond_read.rs 201; gateway/contract.rs 977 и venue.rs 700 (+pool_cond_stat) — давно сверх.
  Дробить: cycle_exec — сборку и пробу (build, below, probe) в cycle_exec_probe.rs; ready.rs — тело запроса (body) в
  ready_body.rs; txs.rs — send_tipped/send_capped в txs_tipped.rs; send/mod.rs — classify в send/classify.rs; track_l1 —
  Start/block_secs/caught_up в track_l1_head.rs. Тестов на записях для решений 436–439 нет — нет записей (docs/TASKS.md,
  «Решение 438», «Решение 439»).
- Решение 437 (3) (`wc -l`): сверх ориентира 200 и до решения — venues/bsc_dex/exec/txs.rs 268 (было 254; send_tipped), venues/bsc_dex/venue.rs 686 (было 676; tip_share / reset_tip_share), gateway/contract.rs 953 (было 932). Разбор проигрыша (exec/tip_judge.rs) и выбор чаевых (exec/tip_pick.rs) без тестов — нет записей блоков Arbitrum с откатом нашей транзакции и перехватчиком (TASKS, «Решение 437»).
- Решение 437 (`wc -l`): сверх ориентира 200 — venues/bsc_dex/send/ready.rs 231 (было 212; send/settle.rs 224 (было 216) — «выброшена» без отмены; настройки секвенсора вынесены в send/seq_opts.rs, остаток — разбор channel.* и тело запроса: вынести body() в send/ready_body.rs). SendCfg::read у сети eip1559_l1 по-прежнему требует send.max_wait_blocks / send.cancel_* / send.bundle_max_blocks, которые трекер этой сети не использует (send/track_l1.rs) — вопрос 1 «Решение 437» в TASKS.
- Решение 436 (`wc -l`): сверх ориентира 200 — venues/bsc_dex/send/ready.rs 212 (было 175; минимум газа и окно условия — можно вынести min_gas в отдельный файл), send/mod.rs 297 (293), exec/txs.rs 252 (246), settings/catalog/dex.rs 215 (214). Тестов на arb_min_gas.rs и sequencer_cond.rs нет — нет записей своего узла Arbitrum (getMinimumGasPrice) и ответа секвенсора (eth_sendRawTransactionConditional) — docs/TASKS.md, «Решение 436». Условная отправка зависит от метода, который Offchain Labs отключает на секвенсоре Arbitrum One 17.01.2027.
- Решение 435 (шаг 5б двух сетей; `wc -l`): сверх ориентира 200 — main.rs 462 (было 418; 457 до правок ревью 435; регистрацию DEX — в venues/bsc_dex/dex_venues.rs целиком), venues/bsc_dex/ctx.rs 210 (207), connection.rs 209 (203), check/pass_cycle.rs 229 (217), exec/bundle_send.rs 299 (293), exec/executor/onchain.rs 245 (237), quote/cycle_audit.rs 266 (253; очередь нарушений — в cycle_audit_queue.rs), node/ws_task.rs 206 (204), pools/families.rs 651 (645), settings/catalog/dex.rs 214 (210); exec/txs.rs стал 246 (было 255). Ошибки узла без Ctx (errors.rs: node/ws.rs, receipts_handle.rs, multicall.rs, rpc_capped.rs, rpc_race.rs, balances_mc.rs, load_batch.rs, ответы WS ws_task.rs) помечаются bsc_dex — у arb_dex перепомечаются только запросы node/rpc.rs (ревью 435: также node/ws.rs request/subscribe и node/call.rs — без Ctx::tag; ядро правила берёт по своей площадке, core/rules_apply.rs, — неверна площадка в тексте ошибки; шапка errors.rs исправлена); тексты «узел BSC», «BNB» в сообщениях узла и предпроверки газа (errors.rs SOURCE, gas_fund.rs) — общие для сетей. dex.venues читается только при запуске (новая площадка — после перезапуска). Ключи DEX при двух площадках читают dex.chain и dex.default_chain на каждый families::all / dex_ids::extra и каждый канал (serves) — при одной площадке чтения нет. Исполнение через KyberSwap (exec.via_kyber) подписывает legacy-транзакции своим кодом — у площадки eip1559_l1 не использовать. Записи SETTINGS_CASES о bsc.* (область) — по долгу settings_cases_legacy.
- Решение 432 (шаг 4 двух сетей): core/risk.rs 352 строк (было 333), core/commands.rs 300 (282), settings/accounts/chain_exec.rs 209, api/wallets_dex.rs 206 (190) — сверх ориентира 200; статики адаптера DEX без площадки (CHECKED, SENT, pool_pass) и ID адаптера константой — до второго экземпляра (шаг 5); остальное — docs/TASKS.md, блок 432.
- Решение 433: папка адаптера называется venues/bsc_dex и тип BscDex, хотя код общий для EVM-сетей (arb_dex — второй экземпляр) — переименование отдельной механической задачей после того, как обе сети заработают (step5-plan.md, раздел 1). Адаптер пока одноэкземплярный: константа ID и общие на процесс переменные без площадки — задачи 433-г, 433-и (docs/TASKS.md).
- Решение 430 (шаг 2 двух сетей): core/transfers.rs 589 строк (было 571 — и до 430 сверх 200), core/native.rs 224 (было 210) — сверх ориентира 200; ключи второй сети (пара venue_dir и область «монета» с сетью), *_bnb → *_native, отображение по сетям (deal_card «bsc_dex», notify/human.rs, экран «Монеты», слепок кошельков), карты ticker_index по адресу без сети — docs/TASKS.md, блок 430.
- Решение 431 (шаг 3 двух сетей): сверх ориентира 200 строк — core/transfers.rs 595, core/native.rs 230, core/rebuild.rs 206, core/precheck.rs 667 (было 655), venues/htx/networks_parse.rs 213 (201), venues/coinex/networks_parse.rs 201; subscribe.rs разнесён (186 + subscribe_assets.rs 117). Не сделано по сетям — docs/TASKS.md, блок 431.
- Решение 415 (t200, после ревью): сверх ориентира 200 и до t200 — core/sell.rs 394 (было 375), core/sell_range_run.rs 363 (было 350), core/relay.rs 338 (было 331), core/stuck.rs 218 (было 215); t200 добавил только ход очереди и строки шапок. tests/core_sell_turn_t200.rs 466 — разнести (LQR / ключи и след / продолжение) с общим помощником.
- Решение 414 (ревью 648177a2): core/drawdown_pending.rs 214 строк — вынести Cur, closing и Closing в
  drawdown_closing.rs (только перенос; пути arb::core::drawdown_pending::{closing, cur} в tests/core_drawdown_414.rs и
  тип в inner.rs — поправить вместе).
- Решение 414 (финальное ревью t199): tests/core_drawdown_414.rs 547 строк — разнести по темам (слепок, затраты
  возврата, переход со старого лимита, перезапуск) с общим помощником в tests/drawdown_common/.
- Решение 414 (повторное ревью t199): сверх ориентира 200 после t199 — core/daily.rs 219 (было 173; дробить: settle /
  settle_close / record — в daily_settle.rs), core/daily_limit.rs 212 (тексты сброса — в daily_limit_text.rs),
  core/daily_venue_limit.rs 205 (reset — в daily_venue_reset.rs), core/ledger.rs 241 (было 238). drawdown_limit.rs
  разнесён (тексты — drawdown_note.rs).
- Решение 414 (ревью t199): сверх ориентира 200 — core/risk.rs 328 (было 281 до t199; t199 добавил паузы по просадке в
  resume_* и snapshot; дробить: snapshot — в risk_view.rs, resume_* — в risk_resume.rs), core/daily_venue_limit.rs 205,
  core/drawdown_limit.rs 201. Чтение котируемой монеты на торговых счетах биржи — дубль core/drawdown_fact.rs usdt и
  core/remain_fact.rs fact (зона t198, не трогалась): после слияния t198 — одна общая функция «котируемая / монета на
  счетах с ролью ≠ Other, free + locked». drawdown_ledger.rs разделён (drawdown_pending.rs). Отчёт «Сделки, лимиты и капитал» (report/cap_limits.rs) — без risk.drawdown_* и
  просадки (вопрос 5 в TASKS). Слепок и учёт своё/чужое читаются в разные моменты (факт счёта, потом учёт сделок):
  покупка, заполненная ровно между ними, учтётся дважды — окно — одно чтение баланса; полностью закрыть — снимать
  слепок под остановкой новых сделок направления на время чтения. Ссылки «каталог base.rs:N» в SETTINGS_CASES для
  настроек ниже строки 19 сдвинулись на 2 (вставлены risk.drawdown_*).
- Решение 413 (финальное ревью t198): src/core/manual.rs — 352 строки (ориентир 200): разнести (алерты, «Продать сейчас», «Беру на себя» — отдельными файлами) отдельной задачей.
- Решение 413 (восьмое ревью t198): src/core/remain_others.rs — 203 строки (ориентир 200): разнести (учёт «Беру на себя» и остатков — отдельным файлом) при следующей правке. Повторы чтения по правилу «повторить» внутри remain_read::with_fact тестом не покрыты (нужен стенд с кодом ошибки XT и правилом шлюза в хранилище правил).
- Решение 413 (пятое ревью t198): остаток «не присвоено» решается в тихий момент счёта; закрытия, кроме продажи (sell.rs), его не ждут — сразу отдельная позиция (ledger::finish → credit_rest::park). Отдельной кнопки «присвоить и продать» / «оставить» нет — это «Продать сейчас» и «Беру на себя» / «Продано вручную» остановленной сделки. Память круга сверки (credit_halted: OBSERVED, NOTED) — только в памяти.
- Решение 413 (повторное ревью t198): круг сверки читает только торговый счёт (balances) — остановленная ждущая с удержанием больше ожидаемого на нём не подтверждается (решит факт всех счетов перед продажей); сверку по account_balances — отдельной задачей. Второе чтение факта между stuck::handle и продажей режима остаётся (шаг меняется, между ними ожидание режима).
- Решение 413 (ревью t198): чтение факта перед продажей / выводом на бирже — account_balances (все счета площадки; у части бирж несколько запросов) — сузить до торгового и перекладываемых счетов отдельным методом контракта. Общий замок продажи (credit_lock.rs) держится на весь trade_seen, чтение факта ждёт его конца без предела (третье ревью, С-А) — сузить до одной отправки ордера можно только внутри пути ордера адаптеров. Совет transfer.deposit_hold_samples (5) — в SETTINGS_CASES, в FIRST_RUN / config/first_run.toml не внесён.
- Решение 413 (t198, 19.11.2026; `wc -l`): сверх ориентира 200 — core/sell.rs ~350 (было 335), core/remain_fact.rs ~245 (было 205), core/transfers.rs ~560, core/credit_apply.rs ~220, core/transfer_credit.rs ~240, core/sell_range_run.rs ~350. Снимок посторонних монет до вывода (core/credit_base.rs) — только в памяти: перезапуск посреди перевода — посторонние 0, замер удержания «неоднозначный». Ячейка снимка у перевода, который не дошёл до зачисления, не удаляется (несколько байт на сделку до перезапуска). Замок счёта (core/credit_lock.rs) не охватывает продажу кнопки «Продать» панели «Балансы» (balance_sell.rs — монеты не сделок). Запись зачисления сделки не показана отдельным полем на экране «Сделки» — только в хронологии.
- Решение 411 (t196, 19.11.2026, переделка «по триггеру»; `wc -l`): сверх ориентира 200 — core/precheck.rs 635 (было
  583; дробить: Timing и строки времени — в precheck_timing.rs, пересчёт и проверка продажи — в precheck_calc.rs),
  core/transfers.rs 546 (было 497; to_address — в transfers_addr.rs), gateway/addr_book.rs 253 (тесты — в tests/),
  core/precheck_par.rs 214. Пометки «спросить адрес заново» (core/deposit_proof.rs DOUBT) — без отпечатка ключа
  (снимаются при смене секретов). Триггер «биржа сообщила о новом адресе» (приватный поток) — не найден, у части бирж
  документация не прочитана (TASKS t196). Gate: баланс в предпроверке — один REST-запрос; снимок баланса по
  spot.balances — после проверки поля `change` на живой записи.
- Решение 410 (t195, 18.11.2026): горячий путь DEX — не сделано (крупнее точечной правки или рискованно): копии Entry в
  `by_pair` с владением дальше — quote/mod.rs:247 (routing::Input.entries: Vec<Entry> → classic_direct, smart_quote),
  quote/fills.rs:107, quote/bound_set.rs pool_set; quote/route_cache.rs fresh — Registry::get (копия) на каждый шаг
  каждого пути при попадании в кэш путей (Step.edge.entry: Entry); копии Entry в ногах пути (Leg / Hop .entry, quote/
  route_calc.rs build, graph_seq.rs, cycle.rs). Правка — шаг пути с Arc записи на время одной котировки (не в графе:
  урок t194 (б)). Кэш sqrt_ratio_at_tick (чистая функция тика) — нужен предел размера: числом в коде нельзя (хардкод),
  настройкой — не цель задачи; getTickAtSqrtRatio — бинарный поиск ~21 вызов sqrt_ratio_at_tick (только swap_final).
  quote/route_cache.rs key — строка ключа с to_string адресов на каждую котировку. Реестр пулов (registry_index.rs) —
  SipHash: обход задаёт порядок снимка и рёбер графа.
- Решение 405 (t191, 18.11.2026; `wc -l`): сверх ориентира 200 — core/coin_ride.rs 318 (было 281), core/manual_wait.rs 225
  (дробить: тексты и list — в manual_wait_view.rs); ручные кнопки при идущих сделках — ожидание не переживает перезапуск
  (команды человека в памяти; если нужно — сохранять ManualCommand ожидания в учёте и поднимать при запуске), core/ride_quote.rs 240
  (238), тест tests/core_coin_reentry_t179.rs 1521 (дробить: общий старт и помощники — в tests/topup_common).
  Резерв USDT (core/usdt_hold.rs) — проход по набору unsettled.rs (идущие и с незаконченным возвратом; стоимость —
  O(размер набора) поисков в учёте под одним замком `deals`, плюс короткий замок ожиданий зачисления, если есть «идёт»);
  «идёт» до хеша вывода от отправителя резервируется поверх уже уменьшившегося свободного (биржа могла списать раньше
  хеша) — покупка берёт меньше. Сверка остатка (core/remain_others.rs): сделка в пути через счёт — «неясно» (решение
  222 без вычета других): если её монеты уже у получателя до учёта зачисления, застрявшая сделка того же счёта может
  их задеть, как до решения 405 (признака «вывод запрошен / монеты пришли» в сделке нет). Застрявшая после
  несостоявшегося перевода (монеты могут быть в возврате вывода) считается лежащей на площадке покупки. Окно маппинга (ревью М4): кошелёк новой
  сделки для правила встречных — по маппингу на момент admit, у сделки он пишется чуть позже (executor.rs). Ручные действия (coin_ride::take: manual_halted.rs:137, :237, manual_return.rs:25,
  balance_sell.rs:50) отказывают, пока у монеты есть хоть одна идущая сделка, — с докупкой и другими направлениями окно
  шире (ревью Н3; вопрос — TASKS, решение 405). Встречное направление на общем счёте не глушится в очереди расчёта
  (blocks_new = false — чтобы причина была в core.start_refused): расчёт идёт и отказывается на входе исполнителя.
  uncapped_watch.rs (наблюдение «без потолка») по-прежнему молчит при любой занятости монеты (coin_ride::held), а не
  по правилу очереди кандидатов (blocks_new) — не тронуто.
- Решение 401 (t186 и ревью, 17.11.2026; `wc -l`): сверх ориентира 200 — exec/executor/entry.rs ≈ 420 (было 368:
  shape / singleton_step / fits_ok), check/pass_cycle.rs ≈ 215; факты пробы через исполнитель (exec/executor/pool_pass.rs)
  — только в памяти: после перезапуска до первой пробы действует приёмка класса (решение 400); пробы — отдельной фоновой
  задачей, последовательно (длительность прохода — docs/TASKS.md, блок 401); чтения настроек каталога на горячем пути
  котировки (Store::cache под RwLock) — общие для всей котировки, не тронуты (вне t186).
- Решение 399 (ревью 5203fdff..76a5fdc8, 17.11.2026): подсказки «переразверните» у v1 / несовпавшей обёртки при
  запрете развёртывания; хрупкость сторожей settings_before_send_guard / core_status_poll_398 /
  core_close_return_stack_399 (точные подстроки и тексты; стековый сторож не доходит до `withdraw`); deploy.rs
  `on_chain_ok(...)?` теряет адрес и tx развёртывания при сбое узла — подробно docs/TASKS.md, блок 399.
- Решение 396 (журнал настроек): старых ключей без записи в docs/SETTINGS_CASES.md — **192** (tests/data/settings_cases_legacy.txt, каталог 141619bc; было 547 до описания пачкой). Остались ключи, появившиеся в каталоге до 01.11.2026 (кроме групп, описанных целиком). В 359 записях: 68 сжатых — описание каталога молчит о поведении без значения (список — docs/TASKS.md, решение 396); поле «Влияет на торговлю» в сжатых записях не заполнено. Список долга только уменьшается: описал ключ — убрать строку из долга.

- Решение 395 (t181): contracts/ArbExecutor.sol 654 строки (было 546; + UNI_V4 / INF_CL / INF_BIN) — сверх ориентира 200, контракт не
  дробится, пока развёртывание — один файл; contracts/evmtest/vendor/ — копии официальных v4-core / infinity-core для
  EVM-теста (≈ 580 КБ исходников; обновлять только вместе с коммитом источника и SHA256SUMS).
- Решение 394 (t179, ревью 3; `wc -l`): сверх ориентира 200 — core/coin_group.rs 250 (было 355: общий возврат вынесен в
  coin_group_flush.rs 175), core/coin_ride.rs 270 (было 240: место сделки при ушедшем общем возврате), core/ledger.rs
  232. Окно «расчёт докупки → admit» (план без фиксированной части возврата, а сделка стала первой новой группы) — не
  закрыто: нужна перепроверка возврата в предпроверке по месту сделки (Seat). Повторов у кусков общего возврата нет
  (docs/TASKS.md, вопрос 6). Тест пути executor целиком (accepted → admit/join → board → Drop ride_guard) и
  return_b_top_up на DirCtx — не написаны (нужны площадки / сборка DirCtx).
- Решение 394 (t179): файлы сверх ориентира 200 строк — core/executor.rs 648 (было 609), core/ledger.rs 219 (203), core/precheck_dex_sell.rs 246 (238), core/candidates.rs 432 (430), core/inner.rs 292 (280), core/costs_source.rs 222 (172), core/credit_event.rs 280 (253), core/usdt_return_deal.rs 277 (253), core/deal_text.rs 210 (204); новые core/coin_ride.rs 228, core/coin_group.rs 228 (дробить: блоки Pool/route и on_saved/flush); тест tests/core_coin_reentry_t179.rs 740.
- Решения 372–377 (t162 и ревью; `wc -l`): core/bound.rs, bound_in.rs, sieve.rs и venues/bsc_dex/quote/bound.rs разделены (все ≤ 200: 159/134, 112/135, 148/132, 129/104/75). Остались больше 200: core/candidates.rs 429 (было 407 до t162; вызов отсева и контроль в calc_one), core/profit.rs 383 (было 367; decide_pre и готовый контекст) — делить при следующей правке; тесты t162 (tests/core_sieve_inputs_t162.rs 270, core_sieve_recorded_t162.rs 293, bsc_dex_bound_quote_t162.rs 257, sieve_common/mod.rs 400) — как остальные тесты на записях; виды пулов без r_e — метод не действует до записей (docs/TASKS.md).
- Решения 358, 359 (t161; `wc -l`): сверх ориентира 200 строк выросли core/dirctx.rs 304 → 321 (сравнение σ у плана входа), core/costs.rs 268 → 278 (запас по σ_T рядом с прежним), core/candidates.rs 341 → 344 (signal_note). Разнести при правке оценки σ (решение 360).
- Решение 308 (t137; размеры по wc -l после пятого ревью): сверх ориентира 200 строк — pools/graph_cache.rs 256 (было 230 до t137: кэш t132 + переключатель режима; после приёмки 308 и отказа от пути t132 — удалить замок building и кэш по Stamp), pools/graph_owner.rs 213, pools/graph_finish.rs 209, pools/graph_alarm.rs 203, pools/graph_stats.rs 203 (вынести Buckets в graph_buckets.rs), pools/graph_snap.rs 202, pools/graph.rs 202, venue.rs ~610 (до правки 578).
- Решение 301 (t132, после слияния с rel/dex-all; `wc -l` на итоговом коде): файлы сверх ориентира 200 строк
  (до t132 → стало) — pools/depth_book.rs 240 → 286 (depth_prep_r, книга неудач depth_fail, счётчик calcs),
  pools/graph_cache.rs 159 → 230 (очередь построений cached_as_of, замер), quote/graph_route.rs 216 → 261 (в релизе
  254 после решения 300; t132 — проверки abandon), pools/registry.rs 174 → 203 (поколение состояния state_gen),
  quote/depth_band.rs 457 → 472 (band_from_units).
- Решение 303, дополнение (t134, ревью): src/venues/bsc_dex/send/mod.rs — 275 строк (было 269 до дополнения; сверх ориентира 200 и до правки: способ канала и предел блока в classify для журнала). Дробить — вынести classify (разбор итогов каналов) в send/classify.rs; reject_log.rs — 107 строк.
- Решение 305 (t135, с ревью; `wc -l`): сверх ориентира 200 — src/venues/bsc_dex/pools/factory_calls.rs 287 строк (было
  247, +40: пары WooFi по quoteToken()), pools/factory_scan.rs 229 (было 216: чтение quoteToken() и счёт вызовов),
  directory/mod.rs 204 (было 199: ban для пары WooFi). В пределах: pools/multi_pair.rs 200, pools/market.rs 189,
  pools/woo_quote.rs 84, tests/bsc_dex_woofi_pairs_retune.rs. Дробить factory_calls.rs — вынести ветку WooFi и
  уровни/шаги (tiers / spacings / lookup_max) в factory_levels.rs; factory_scan.rs — sort / off / failed (см. 292).
- Решение 296 (t127): тест на записи SHOPINX/WBNB (fit_volume_2026-11-15, ветка t125) — после её слияния (TASKS 296 п. 2); off_map перецентрирует карту только по полосе глубины, не по полосе чтения (TASKS 296 п. 4). Размеры: load_v2v3.rs 156, load_algebra.rs 120 строк — в ориентире.
- Решение 292 (t123, с ревью): src/venues/bsc_dex/pools/factory_scan.rs 216 строк (было 207; вызов источника
  stable_list.rs и счёт вызовов) — сверх ориентира 200 и до правки; pools/stable_list.rs 211 (ревью: разбор пачки sort
  вынесен функцией) — сверх ориентира; tests/bsc_dex_stable_list_live.rs 188, tests/bsc_dex_stable_list_sort.rs 144,
  load_saddle.rs 181, load_curve.rs 150. Дробить factory_scan.rs — вынести sort / off / failed в отдельный файл;
  stable_list.rs — вынести sort (разбор пачки) в stable_list_sort.rs.
- Решение 293 (t124, с двумя ревью; `wc -l` на итоговом коде): src/venues/bingx/symbols.rs — 222 строки (было 215, сверх ориентира 200 и до правки: поле qty_store, вызов take / warn_bad); дальше дробить — вынести parse / api_flag в symbols_parse.rs (как у других бирж). core/sell_unsent.rs — 243 (было 237), core/ledger.rs — 203 (было 211: пауза вынесена в route_pause.rs); тест tests/core_sell_reject_293.rs — 326 (сверх ориентира). В пределах 200 — symbols_qty.rs 166, pairs.rs 155, pairs_watch.rs 127, route_pause.rs 97, settings/catalog/bingx.rs, тесты bingx_minqty_293.rs 128, bingx_minqty_coin_293.rs 143, bingx_293_common/mod.rs 174.
- Решение 294 (t125; `wc -l`): в пределах 200 — новые quote/route_fit.rs 184, settings/catalog/dex_fit.rs 16,
  tests/fit_common/mod.rs 167, tests/core_fit_volume_294.rs 116, tests/bsc_dex_fit_graph_294.rs 187; quote/via_pools.rs
  189 (было 154), quote/routing.rs 148 (было 138), settings/catalog/mod.rs 177 (было 175). Сверх ориентира —
  quote/graph_route.rs 216 (было 210 и до правки: запись «граф не дал маршрут» вынесена в failed, общая с
  route_fit.rs), tests/route_common/mod.rs 465 (было 459: load / v3 / v2 / ctx стали pub). route_fit.rs повторяет
  сборку хопов Via::calc (8 строк) — via.rs в этой задаче не трогался; общий сборщик хопов — отдельной правкой.
- Задача t122 (с ревью; `wc -l` после всех правок): ui/src/components/SettingCard.svelte — 201 строка (сверх ориентира
  200; код не менялся); выросли сверх ориентира — exec/dry_build.rs 206 (было 202: причина native_block и общий текст
  native_refusal), tests/bsc_dex_head_stall.rs 296 (было 246: тест «событие не ушло»), node/heads.rs 257 (было 246:
  порог до fresh(), rustfmt вызова step_with); exec/executor/trade.rs 271 (было 272). В пределах 200 — новые
  exec/executor/callback_at.rs 55 и node/head_stall_send.rs 58 (отправка вынесена из head_stall.rs), node/head_stall.rs
  185, exec/executor/mod.rs 137, check/probe_exec.rs 164.
- Решение 283 (t116): src/settings/schema.rs 210 строк (было 197; поле venue_for и .dex_venue()). Перечисление VenueFor
  вынесено в settings/venue_for.rs; дальше дробить — вынести билдеры SettingDef (impl) в отдельный файл.
- Задача t114: exec/executor/trade.rs 268 строк (было 259; причина native_block и повторная проверка обратного вызова
  после сверки), exec/executor/deploy.rs 205 (было 171; confirm и его тест), contracts/ArbExecutor.sol 452 (было 448;
  squadV3SwapCallback) — сверх ориентира 200; trade.rs и ArbExecutor.sol были сверх и до правки.
- Решение 282 (t115): src/venues/bsc_dex/node/heads.rs 246 строк (было 237 — сверх ориентира 200 и до правки: +поле stall и вызов шага уведомления); новые node/head_stall.rs (164) и head_stall_text.rs (60) — в пределах 200; тест tests/bsc_dex_head_stall.rs 246 — сверх ориентира (после rustfmt).
- Решение 295 (t126, все пары многомонетного пула): файлы сверх ориентира 200 строк (`wc -l` до → после): pools/load_saddle.rs
  182 → 230 (read / state для одного чтения на пул), pools/load_stable_u.rs 190 → 261 (тело load разделено на части
  answers / head / indices / rates / finish, порядок шагов прежний), pools/load_batch.rs 235 → 250, pools/stable_list.rs
  211 → 222, pools/verify_balancer.rs 200 → 213, pools/verify_stable_u.rs 214 → 218, pools/families.rs 619 → 633,
  pools/load.rs 481 → 487, pools/created_log.rs 342 → 346, pools/factory_scan.rs 216 → 220, pools/session.rs 237 → 242,
  quote/route_paths.rs 241 → 266, quote/route_best.rs 235 → 249, quote/graph_seq.rs 200 → 204, quote/graph_split.rs
  261 → 263, quote/quoter_calls.rs 441 → 443, core/types/pool.rs 395 → 427. Новые файлы кода и тестов — в пределах
  200 (multi_pair.rs, load_site.rs, listen_site.rs, index_pass.rs; tests/bsc_dex_multi_pair*.rs). Дробить: read / state
  загрузчиков — в load_site.rs (с Raw), части load_stable_u — в load_stable_u_parts.rs.
- Ревью 289 (Balancer V2): (а) ~~пул с 3–6 токенами даёт в графе одну пару~~ — закрыто решением 295 (`dex.multi_pair`);
  было: пул с 3–6 токенами даёт в графе одну пару — справочник берёт у пула пару base/quote
  индексатора и один кандидат на адрес (pools/index.rs L78), реестр — одна запись на ссылку (registry_index.rs L89-L95);
  все пары — только сменой общего ключа справочника/реестра (своя ссылка «пул + пара» для VaultPool и чтение пула один раз
  на блок на пул) — отдельная задача; (б) память неизменяемого пула FIXED (load_balancer_fixed.rs) живёт до перезапуска,
  как N_COINS в load_curve_ng.rs; (в) tests/bsc_dex_balancer_acsi_live.rs — 521 строк (сверх ориентира 200).
- Решение 289 (Balancer V2 / ACSI Finance, t120): (а) новые пулы — только из справочника индексатора: подписка на PoolRegistered / TokensRegistered хранилища (лог от адреса Vault, токены — отдельным событием) не подключена; (б) балансы меняют события хранилища (Swap / PoolBalanceChanged / PoolBalanceManaged с id пула в topic1) — вид перечитывается каждым блоком (ревью 289: все пулы вида — одной пачкой на блок, per_block_balancer.rs); слушать события хранилища по id пула — дешевле, нужен разбор лога Vault по topic1; (в) состояние после обмена доли (pool_after.rs) для VaultPool не считается — вторая доля через тот же пул не допускается; (г) 3 пула acsi-finance с кодом 19 408 байт (0xb978bd52, 0x86ddc49f, 0xdaae64e8; ≈1,2 тыс. USD) — не из фабрик, исходник не установлен, не берутся; (д) файлы сверх ориентира 200 строк — pools/families.rs 605 (было 591), pools/load.rs 472 (460), pools/verify.rs 318 (311), pools/created_log.rs 338 (334), pools/load_batch.rs 234 (233), quote/pool_quote.rs 507 (495), quote/depth_band.rs 445 (438), quote/quoter_calls.rs 434 (427), quote/mod.rs 271 (269), quote/steps.rs 228 (224), exec/executor/entry.rs 232 (220), core/types/pool.rs 386 (377), contracts/ArbExecutor.sol 536 (510), тесты tests/bsc_dex_executor_abi.rs 374 (367) и новый tests/bsc_dex_balancer_acsi_live.rs 341 (после rustfmt); новые файлы кода — в пределах 200 (verify_balancer.rs — 200, balancer_log_exp.rs — 199).
- Решение 290 (Liquidity Book V2.1 / LFJ, t121): (а) новые пулы — только из справочника индексатора: подписка на LBPairCreated(tokenX, tokenY, binStep — indexed, LBPair, pid) фабрики (ILBFactory.sol L49-L51) не подключена; (б) у пары без USDT стоимость бинов в USDT не считается — читаются все бины (последовательные getNextNonEmptyBin / getBin; только при чтении и на событие пула); оценка через промежуточную монету — отдельная задача; (в) состояние после обмена доли (pool_after.rs) для LbBin не считается — вторая доля через тот же пул не допускается; (г) точный выход (getSwapIn) не портирован — только точный вход; ревью 290: котировка — на времени блока чтения, сделка — в следующем блоке, изменчивая комиссия к нему может упасть или вырасти, защищает только minOut; на событие пула — только затронутые бины одной пачкой (load_lb_bins::many), но первое чтение пары без USDT — все бины последовательно (≈477 eth_call у 0x722e…912f); (д) файлы сверх ориентира 200 строк — pools/families.rs 618 (было 605), pools/load.rs 481 (472), pools/verify.rs 325 (318), pools/created_log.rs 342 (338), pools/load_batch.rs 235 (234), pools/exec_kind.rs 202 (194), quote/pool_quote.rs 524 (507), quote/depth_band.rs 457 (445), quote/quoter_calls.rs 441 (434), quote/mod.rs 272 (271), quote/steps.rs 233 (228), exec/executor/entry.rs 241 (232), core/types/pool.rs 395 (386), contracts/ArbExecutor.sol 546 (536), contracts/IPools.sol 205 (195), тесты tests/bsc_dex_executor_abi.rs 376 (374) и новый tests/bsc_dex_lb_v21_live.rs 315 (после rustfmt); новые файлы кода — в пределах 200.
- Решение 291 (four.meme, t121): не подключается (DECISIONS 291) — у монеты на кривой нет второй площадки, после миграции кривая закрыта; исходник TokenManager2 закрыт. Пересмотр — по решению Тиграна или при факте монеты на кривой и на бирже бота одновременно (TASKS 291 п. 1).
- Решение 288 (SmarDex, t119): (а) глубина полосы пары SmarDex — оценка x·y=k по фиктивным резервам (деление обмена на
  два шага по средней цене не учитывается); (б) котировка — на времени блока чтения: замерить на живых сделках расхождение
  факта с котировкой из-за сдвига средней цены к следующему блоку; (в) состояние после обмена доли (pool_after.rs) для
  Fictive не считается — вторая доля через ту же пару не допускается; (г) файлы сверх ориентира 200 строк —
  pools/load.rs 460 (было 448), pools/families.rs 591 (было 579), quote/quoter_calls.rs 427 (было 420),
  quote/pool_quote.rs 495 (было 485), quote/depth_band.rs 438 (было 414), quote/virtual_quoter.rs 233 (было 225),
  quote/steps.rs 224 (было 220), pools/created_log.rs 334, pools/factory_calls.rs 250, pools/load_batch.rs 233,
  exec/executor/entry.rs 220 (было 211), core/types/pool.rs 377 (было 369), contracts/ArbExecutor.sol 510 (было 502);
  новые файлы кода — в пределах 200.
- Решение 287 (ревью): котировка пула DMM — по комиссии getTradeInfo блока чтения, исполнение — по комиссии блока
  исполнения (меняется каждым блоком без событий); от сдвига защищает только minOut маршрута — замерить на живых сделках
  и при необходимости учесть запас по комиссии (TASKS 287, п. 5).
- Решение 287 (KyberSwap Classic DMM, t119): (а) пулы KSFactory со статической комиссией (KyberSwap Classic «Static Fee»,
  фабрика 0x1c758af0688502e49140230f6b0ebd376d429be5 по официальной документации; в справочнике GeckoTerminal
  `kyberswap_classic_bsc` их 2 из 13) — другой вид пула (KSPool), не подключён: проверка видом kyber_dmm их отклоняет по
  factory(); (б) состояние пула после обмена доли (pool_after.rs) для AmpV2 не считается — вторая доля через тот же пул
  DMM не допускается (как у прочих видов кроме V2/V3); (в) файлы сверх ориентира 200 строк — pools/load.rs 448 (было 436),
  pools/families.rs 579 (было 567), quote/quoter_calls.rs 420 (было 413), quote/pool_quote.rs 485 (было 469),
  quote/depth_band.rs 414 (было 387), pools/created_log.rs 333 (было 320), pools/factory_ask.rs 217 (было 202),
  pools/factory_calls.rs 249 (было 247), pools/load_batch.rs 232 (было 231), quote/steps.rs 220 (было 216),
  exec/executor/entry.rs 211 (было 203), core/types/pool.rs 369 (было 362), contracts/ArbExecutor.sol 502 (было 482);
  новые файлы кода — в пределах 200.
- Решение 286 (iZiSwap / Biswap V3, t118): (а) подписка на NewPool фабрик и поиск пулов по реестру pool(x, y, fee) не
  подключены — новые пулы только из справочника индексатора (created_log.rs, как Metric); (б) лесенка — только пары с USDT
  (load_metric_ladder.rs: суммы dex.pmm_ladder_usd в USDT): на странице 1 GeckoTerminal у biswap-v3-1 9 из 20 пулов без
  USDT (XRP/WBNB, BTCB/WBNB, ETH/BTCB, ETH/WBNB, SOL/WBNB, DOGE/WBNB, ETC/WBNB, BSW/WBNB, WBNB/BUSD) — такие пулы без
  котировки; своей математики точек iZiSwap (порт SwapMathX2Y/Y2X, точки и лимитные ордера) нет; (в) лесенка каждым
  блоком — как у Metric (последовательно в per_block::on_head; ревью t118 — только пулы с USDT, per_block::each_block):
  замерить на сервере после выкладки длительность on_head — на каждый пул iZiSwap / Biswap V3 с USDT за блок 1–3 + 2N
  eth_call (N — точек dex.pmm_ladder_usd), пулы по очереди; (г) файлы сверх ориентира 200 строк — pools/load.rs 436
  (было 421), pools/families.rs 567 (было 545), quote/quoter_calls.rs 413 (было 406), pools/verify.rs 311 (было 304),
  pools/load_batch.rs 231 (было 230), pools/created_log.rs 320 (было 317), quote/virtual_quoter.rs 225 (было 204),
  contracts/ArbExecutor.sol 482 (было 470); тест tests/bsc_dex_izi_live.rs — 214 (после rustfmt). Новые файлы кода — в
  пределах 200.
- Решение 285 (ревью t117): лесенка Metric — полное перечитывание каждого пула на каждом блоке последовательно в
  per_block::on_head; задержка не мерилась — замер на сервере после выкладки (docs/TASKS.md, решение 285, п. 7).
- Решение 285 (Metric OMM, t117): файлы сверх ориентира 200 строк — pools/load.rs 421 (было 406), pools/families.rs 545
  (было 532), quote/quoter_calls.rs 406 (было 399), pools/verify.rs 304 (было 294), pools/load_batch.rs 230 (было 229),
  quote/virtual_quoter.rs 204 (было 173), contracts/ArbExecutor.sol 470 (было 458). Новые файлы — в пределах 200.
- Решение 284 (Nomiswap Stable, t117): файлы сверх ориентира 200 строк, выросшие в ветке — src/venues/bsc_dex/pools/load.rs
  406 (было 391: ветка Proto::NomiStable), pools/families.rs 532 (было 520), quote/pool_quote.rs 469 (было 456),
  quote/quoter_calls.rs 399 (было 382), contracts/ArbExecutor.sol 458 (было 452). Новые файлы — в пределах 200.
- Решение 280 (t112): src/venues/bsc_dex/pools/families.rs 499 строк (было 486), check/probe.rs 261 (было 257),
  check/taxes.rs 224 (было 218) — все были сверх ориентира 200 и до правки; новые файлы quote/v2_pair_{quote,sim,probe}.rs
  уложены в 200. Ревью 280: exec/bundle.rs 361 (было 349; отказ ноги без своего пути), check/taxes.rs 228 (было 224);
  quote/v2_pair_{quote,sim,probe,calls}.rs, check/pool_taxes.rs и новые тесты — в пределах 200.
- Решение 281 (SquadSwap V3, t113): файлы сверх ориентира 200 строк, выросшие в ветке — src/venues/bsc_dex/pools/load.rs
  391 (было 361: поле Loaded.fee_pips в 13 литералах + чтение fee()), pools/load_batch.rs 229 (было 219),
  pools/verify.rs 294 (было 278), quote/quoter_calls.rs 382 (было 375). Делить — отдельной задачей (load.rs — ветки
  видов в свои файлы; Loaded — конструктор вместо литералов). Ревью 281: pools/load_batch.rs осталась 229 (Extras
  вместо второго чтения настройки); тесты tests/bsc_dex_squadswap_v3_live.rs 313 и tests/bsc_dex_batch_load.rs (+4) — сверх ориентира.
- Решение 271 (дополнение, t108): src/venues/bsc_dex/exec/bundle.rs 349 строк (было 330), exec/bundle_send.rs 269
  (было 263) — оба были сверх ориентира 200 и до правки; quote/graph_route.rs уложен в 200 (decimals → graph_legs.rs).
- Решения 274/275 (SheepDex, Curve NG, с доработками по ревью 14.11): файлы сверх ориентира 200 строк, выросшие в
  ветке t107 — src/venues/bsc_dex/math/stableswap_ng.rs 204 (было 200), src/venues/bsc_dex/pools/families.rs 486,
  src/venues/bsc_dex/pools/load.rs 361, src/venues/bsc_dex/pools/verify.rs 278 (проба swapFee — +9),
  src/venues/bsc_dex/pools/created_log.rs 313, src/venues/bsc_dex/quote/quoter_calls.rs 375. Делить — отдельной
  задачей (families.rs — список required() по видам; verify.rs — ветки видов в свои файлы).
- Решение 273 (с ревью): файлы сверх ориентира 200 строк (было до решения 273 → стало) — quote/mod.rs 239 → 263,
  quote/select.rs 221 → 231, quote/steps.rs 213 → 216, venue.rs 537 → 539, exec/executor/trade.rs 221 → 226,
  exec/txs.rs 212 → 231 (send_capped), quote/pool_quote.rs 454 → 449 (все были сверх ориентира и до решения; пути в
  src/venues/bsc_dex/). Новые файлы в пределах: pools/solidly_fee_read.rs 162, pools/solidly_gate_read.rs 95,
  pools/solidly_refresh.rs 107, exec/solidly_gate.rs 94, math/solidly_fee.rs 95, core/types/pool_solidly.rs 69; тест
  разнесён (tests/bsc_dex_solidly_fee_273.rs 156, _cfg.rs 173, tests/solidly273_common/mod.rs 114). Открытые пункты — docs/TASKS.md,
  блок 273 (п. 1–7): исходник пула magicfox; канал с минимумом газа выше maxGasPrice usdfi; событие паузы не слушается;
  живой сделки нет; плановая и фактическая цена газа расходятся только во времени; граф строится без учёта паузы/предела
  (dex.route_max_pools_per_edge); пути вне выбора котировки и поиска циклов — без отбора по plan_gas.
- Решение 272: contracts/ArbExecutor.sol — 420 строк (было 319, уже сверх ориентира 200): executeNative (нативный BNB
  монетой сделки) добавлен в тот же контракт — развёртывание одним файлом; дробить на библиотеки — отдельное решение
  (меняет байткод и развёртывание). Сверх ориентира после решения: exec/executor/trade.rs 246 (было 221),
  exec/dry_sim.rs 212 (было 208), call.rs 203 (было 149), native_probe.rs 206 (было 197), dry_swap.rs 205 (было 201), dry_build.rs
  202 (было 196); tests/bsc_dex_executor_v2.rs 300. Также (ревью 272): quote/graph_route.rs 229 (было 219 до решения),
  storage/db.rs 230 (было 226).
- Ревью 272: contracts/ArbExecutor.sol 448 (было 420); exec/executor/onchain.rs 209 (было 156; обёртка из слота, память
  с восстановлением записи), code.rs 204 (было 182; слот wnative, код с аргументом конструктора), trade.rs 254 (было
  246), check/native_probe.rs 222 (было 206), venue.rs 574 (было 537 — и так сверх); tests/bsc_dex_executor_v2.rs 326,
  tests/bsc_dex_executor_v2_review.rs 515 (один бинарник теста — сборка одним линковщиком, памяти мало);
  contracts/evmtest/review.mjs 201.
- Решение 268: src/core/sell.rs 335 строк (было 314), src/core/trade_ops.rs 456 (было 435), src/core/sell_range_run.rs 328,
  src/core/sell_range.rs 202, src/core/deal.rs 249 (было 199), src/core/rules_apply.rs 236 (было 217) — sell.rs,
  trade_ops.rs и rules_apply.rs были сверх ориентира 200 и до решения. У 14 адаптеров trade (trade.rs) принимает
  готовый ордер рыночной продажи (prepared) — общий путь постановки и итога лимитного и рыночного ордера, лимитный
  путь без лишнего уровня вызова. Адаптеры Gate и Binance читают настройку ядра sell.market_on_price_range
  (order_state.rs: состояние «ордер отменён защитой цены» без исполнения — код PriceOutOfRange только при включённой
  настройке и только у продажи). Открытые вопросы — docs/TASKS.md.
  Стек в отладочной сборке (замер 14.11, порог RUST_MIN_STACK): tests/core_exec_book_238.rs
  stuck_without_book_stale_ms_halts_at_once проходит при ≈ 2015 КиБ на ef2c8a4 и при ≈ 2025 КиБ после решения
  (поток теста по умолчанию — 2048 КиБ, запас 23 КиБ вместо 33); xter_case_sells_by_rest_without_empty_retries —
  ≈ 1287 и ≈ 1293 КиБ. Продажа кнопкой с отказом лимитного ордера — ≈ 2.4 МиБ на ef2c8a4 (тестовый поток по
  умолчанию переполняется и без решения 268; tests/core_market_sell_268.rs идёт в потоке 16 МиБ). Вложенные будущие
  sell → stuck → relay создаются на стеке. Ветка решения 268 собрана так, чтобы состояние задач sell / trade_seen
  росло минимально: будущие — в коробках из не-async функций, итог ветки пишется в место итога лимитного ордера,
  одна точка ожидания ордера, отметки сделки — одно поле Deal.range (Option<Box<RangeMarks>>). Остаток роста —
  условие продажи (Rule) в задаче sell и поле Deal.range.
  Стек в release (замер 14.11: `cargo test --release`, профиль release по умолчанию — в Cargo.toml своего профиля нет,
  CARGO_PROFILE_RELEASE_DEBUG=0; порог — перебором размера стека потока, на котором идёт продажа): tests/core_exec_book_238.rs
  stuck_without_book_stale_ms_halts_at_once — ≈ 371 КиБ и на ef2c8a4, и после решения;
  xter_case_sells_by_rest_without_empty_retries — ≈ 339 КиБ и там, и там. Пути решения 268
  (tests/core_market_sell_268.rs, размер стека потока теста на время замера задавался переменной окружения):
  entry_unprofitable_goes_to_mode_then_mode_sells_market — ≈ 730 КиБ,
  button_goes_market_without_condition_and_second_refusal_alerts — ≈ 826 КиБ, mode_sale_within_threshold_goes_market_once
  — ≈ 710 КиБ, market_filled_closes_deal_with_proceeds — ≈ 419 КиБ. Запас до 2048 КиБ (поток tokio по умолчанию) у
  самого глубокого из замеренных — ≈ 1222 КиБ. В замер входит и подготовка теста (запуск ядра) на том же потоке.
- Решение 267: src/core/busy_calc.rs 342 строки, src/core/busy_sum.rs 263, src/core/window.rs 271 (было 251),
  src/core/candidates.rs 341 (было 317) — сверх ориентира 200. Сводки замера «пока монета занята сделкой» живут в
  памяти: после перезапуска бота у незакрытой сделки они теряются (строки журнала нет; в окне — то, что было к
  последнему сохранению сводки окна); живого сообщения экрану «Окна» на новый теневой расчёт нет (строка обновляется
  при следующем сохранении сводки окна и при перечитывании списка). Конец занятости монеты замер ловит сверкой
  inner.active раз в window.busy_calc_min_ms (исполнитель не трогался) — после приёмки можно заменить сигналом от
  исполнителя. Тестами на записанных данных не покрыт теневой расчёт, прошедший пороги, на настоящем ядре (полный
  расчёт ядра на записях не собирается — как у решения 266): сводка с прошедшими расчётами проверена на расчётах
  CAKE (tests/size_common), путь ядра — на отказе расчёта; проход — проверяется на живой сделке по строке
  core.busy_calc.
- Решение 266: файлы сверх ориентира 200 строк — src/core/precheck.rs 520 (было 308: ветка «одновременно», строка
  «План», решение удержаний, справочник сетей из памяти, шаг и разбор свободного баланса), src/core/executor.rs 517
  (было 458: ветки «баланс прочитан предпроверкой»), src/core/profit.rs 354 (было 333), src/core/precheck_withdraw.rs
  284 (было 194: fetch и settle рядом с прежней check — прежняя оставлена нетронутой для выключенного режима; после
  приёмки — свести check к fetch + settle), src/core/registry_sync.rs 240 (было 214), новые src/core/precheck_fast.rs
  263 и src/core/precheck_par.rs 218. Делить precheck.rs (шаги сторон, расчёт, удержания) — отдельной задачей после
  приёмки. Тестами на записанных данных не покрыты (полный расчёт ядра, ответы бирж и балансы на записях не
  собираются): проход быстрого пути целиком (финальный расчёт), часть 4 с полученным
  балансом (хватает / под остаток / исполнитель без повторного чтения) — проверяются на живых сделках по строкам
  «План: …», «Перед покупкой свободно…», «время: …». Отказы предпроверки открытого окна (решение 266) — в памяти до закрытия окна: после
  перезапуска бота у незакрытого окна они теряются; живого сообщения экрану на отказ нет. Пауза направления после
  отказа покупки «не хватает средств» проверена тестом чистой функции решения (ledger::finish_paused без паузы —
  без теста: состояние паузы сигнала снаружи не читается).
- Решение 265: файлы сверх ориентира 200 строк — src/core/executor.rs 458 (было 376 до замера времени),
  src/core/profit.rs 333, src/core/precheck.rs 308 (было 262), src/core/costs.rs 268, src/core/calc_timing.rs 205,
  src/gateway/http.rs 202; делить executor по шагам (вход, покупка, перевод) — отдельной задачей. Bybit (свой
  HTTP-обмен) в журнал запросов шага (gateway/step_meter.rs) не пишет.
- Решение 238: строка хронологии «Котировка продажи … (котировке N мс)» показывает возраст ПОСЛЕДНЕГО ИЗМЕНЁННОГО снимка стакана (Quote.at = received_at), а не время последнего кадра пары — у тихой пары это не мера свежести (свежесть — в части «худшая цена ордера … по стакану / снимку REST»). Файлы чуть больше ориентира 200 строк после выделения frozen(): venues/{xt,coinex,kucoin,okx,gate,binance}/book.rs (209–214), core/trade_ops.rs (295). Bitget: ветка снимка REST в quote (решение 201) ядром больше не вызывается (ордер — quote_rest), оставлена для совместимости — убрать отдельной задачей.
- Решение 236: дыры факта итога сделки вне возврата USDT (docs/notes/deal-accounting-2026-11-09.md, вопрос 6 в TASKS):
  газ approve не доходит до ядра (venues/bsc_dex/exec/swap_send.rs send_approvals), газ откатившегося / отменённого
  свопа сделки биржа ↔ DEX — только в gas_stats (core/trade_ops.rs), чаевые канала (chain.tip_bnb) не в TradeResult.fees;
  старые записи возврата без «ушло» — затраты сверху (остаток округления MEXC, вопрос 1). src/core/usdt_return.rs —
  210 строк (ориентир 200).
- Решение 223: доля входа исполнителя — uint16 bps от остатка (вниз, call.rs делит на плановый остаток): у 0.3/0.3/0.3/0.1 сеть проводит 0.3/0.29995/0.3000375/0.1000125 — доли покомпонентно не равны котировке (итог — второй порядок). Форки V2: модель «резервы = балансы после swap» по коду UniswapV2Pair — по каждому форку не сверялась (защита — minOut)
- Плавающий тест tests/core_usdt_return_close.rs::relay_sold_here_close_starts_return_and_failure_alarms — упал 1 раз при полном прогоне под нагрузкой (04.11), 3 из 3 повторов проходят: ожидание по времени, переписать на ожидание события.
- **Профиль процессора 04.11 (BingX + Gate, 1,4–1,8 ядра из 24, без блокировок):** 25 % memcpy, 7 % String::clone, 2–5 % выделения mimalloc, Tokens::usdt + Vec<Token>::clone (venues/bsc_dex/tokens.rs — копия списка токенов на вызов), settings Store::all (обход всей карты) и Store::get_exact (SipHash над (String, Scope)) на горячем пути, k256 lincomb (восстановление/проверка подписи — выяснить, где). Резерв для роста бирж: убрать копии строк и списков токенов, кэш настроек горячего пути без хеширования строк.

Отложенное, не забыть (с номером решения):

- **Решение 138 — возврат USDT после закрытия.** (1) Сквозного теста продажи → возврата через площадку нет (в тестах
  реестр пуст, моков нет): гарантия — общий шаг ledger::finish и страж по исходникам; дописать на записанном живом
  эпизоде, когда будет реплей исполнения. (2) Возврат «идёт» без срока: зависший перегон виден только строкой «идёт» на
  экране сделки — таймаута нет (вывод мог уйти, решать — Тиграну). (3) Кнопки «Перегнать выручку» у закрытой сделки нет
  (вопрос в TASKS). (4) Остальные `tokio::spawn` на пути сделки (manual.rs, manual_halted.rs, manual_return.rs —
  продажа по кнопке) тоже идут без класса Trade — проверить и перевести.

- **Решение 217 — снимок рынка для «без потолка» по сделке.** (1) Стоимость снимка в пути сделки не замерена на живом
  боте (оценка: копия стакана — мкс, `Arc` десятков пулов монеты — мкс; после исправлений по проверке и в graph-режиме
  — только пулы путей монеты из кэша графа, не весь реестр); замерить по времени шагов сделки после обновления.
  (2) Пока снимок жив (до конца фонового расчёта, с ожиданием в очереди), первая запись в каждый снятый пул копирует
  его запись (у V3/CL — и карту тиков при Mint/Burn) — только у десятков снятых пулов. (3) Graph-режим без путей монеты
  в кэше (dex.route_paths_ttl_ms не задана или набор устарел) — снимок минимальный, расчёт может быть занижен (текст
  снимка это говорит); задать dex.route_paths_ttl_ms на живом боте. (6) Bitget: снимок REST котировки исполнения
  годен не старше feed.stale_after_ms (своей настройки нет); у пары со стаканом потока, но молчащим соединением снимок
  — «не посчитано». Теста book_decided.rs нет: ставку комиссии (fees.rs) в тесте не задать без записи all-trade-rate.
  (4) Разрешения кошелька, которых не было в памяти в момент снимка, котировка по снимку
  читает с узла (живые). (5) Старые записи сделок не пересчитываются — данных момента решения у них нет.

- **Решение 141 — расчёт «без потолка».** (1) Сделка, запущенная, но не дошедшая до покупки, не пишется. (2) Для
  прошедших без сделки поиск без бюджета считается дважды — карточкой (card_max.rs) и «без потолка»; объединить по ответу
  Тиграна. (3) Итоги экрана читают все строки периода и считают в памяти (без агрегации SQL) — при десятках тысяч строк
  за период перейти на столбцы чисел и SUM. (4) Минимум до плюса — бисекция вниз от первого проходящего среди
  посчитанных: участок прибыльности уже шага сетки ниже может быть пропущен (как «наибольший объём» карточки).
  (5) executor.rs и candidates.rs — больше 200 строк (были до решения 141).
- **Решение 135 — Gate (ветка router/gate).** (1) Пауза после 429 по X-Gate-RateLimit-Reset-Timestamp — нужен заголовок из gateway/http.rs (стык); сейчас venue.rate_limit_pause_ms. (2) Приватные ответы (withdraw_status, account/detail, main_keys, spot/accounts, ордер ioc, вывод, депозит, spot.balances) — записать на живом боте и снять #[ignore] в tests/gate_real_data.rs. (3) Замер нагрузки при «Стоп» (решение 109) до торговли. (4) Ставки комиссии — по одной паре /wallet/fee; перейти на /spot/batch_fee, когда формат списка пар подтвердят. (5) Файлы чуть больше ориентира 200 строк после rustfmt (mod.rs — как у MEXC, feed.rs, order_state.rs, user_stream.rs) — разделить при следующей правке.
- **Решение 136 — Bybit.** (1) venues/bybit/mod.rs — 239 строк (как mexc/mod.rs): разнести при правке контракта. (2) Свой
  HTTP-обмен venues/bybit/http.rs повторяет gateway/http.rs ради заголовков X-Bapi-Limit* — при правке стыка дать Reply шлюза
  заголовки и убрать копию. (3) Приватные тесты #[ignore] до живых записей. (4) Процентная комиссия вывода (withdrawPercentageFee)
  в ядре не моделируется. (5) Строгая проверка u (+1) — после ответа Тиграна.
- **Решение 137 — KuCoin.** (1) Пауза пула после 429000 — на всё окно 30 с: шлюз не отдаёт заголовок gw-ratelimit-reset
  (запрос на изменение стыка gateway/http.rs). (2) События ордеров /spotMarket/tradeOrdersV2 только пишутся в сырой
  журнал — итог ордера по REST; разобрать после живой записи. (3) venues/kucoin/mod.rs — 242 строки (контракт целиком,
  как у MEXC 237) — разнести вместе с контрактом. (4) Тесты приватных ответов и /market/match — #[ignore] до записи.
  (5) Замер нагрузки при «Стопе» (решение 109) не сделан — до включения площадки.

- **Решение 132 — маппинг кошельков (ветка router/wallets).** (1) Кошельки и маппинг не входят в экспорт/импорт настроек (`settings_io`): после импорта полного бэкапа записи кошельков и строки задаются заново (ключи переносятся в зашифрованном файле). (2) Перегон USDT «отдельно» — только биржа ↔ её кошелёк по маппингу; перегона между кошельками DEX нет. (3) Кэш квот DEX тика (`core/quote_cache.rs`) — по площадке и монете, без счёта: котировка, посчитанная для одного кошелька, может достаться другому (влияет только оценка газа разрешений). (4) Вид адаптера на счёт создаётся на каждый вызов шлюза (`venue_account.rs`) — дешёвые копии Arc, без кэша. (5) Пробы налогов идут через исполнитель кошелька строки «пробы»; смена исполнителя не будит пробы сама (кнопка «Перепроверить»). (6) Размеры файлов > 200 после решения: core/executor.rs (268), core/transfers.rs (236), venues/bsc_dex/venue.rs (423), gateway/contract.rs (444), settings/accounts/edit_map.rs (211).
- **Решение 134 — проверка продажи до покупки.** (1) После предпроверки исполнитель может подобрать объём заново под свободный остаток (core/refit.rs, решение 103) — проверка продажи на новый объём не повторяется (правила стороны и типа от объёма не зависят, минимумы пересчитывает расчёт, но пробный ордер MEXC — только на объём предпроверки); (2) покупка на бирже A через check_order не проверяется — её отказ и так до траты денег; (3) у спота BingX пробного ордера нет — PERCENT_PRICE-подобных фильтров BingX не описано, проверяются только правила справочника пар; (4) время шага — один REST MEXC в предпроверке (замер в журнале), параллельно с другими шагами не запускается.
- **Решение 131 (после 129) — оставшийся CPU с MEXC** (разбор — docs/notes/mexc-cpu-starve-2026-11-03.md, раздел «Что осталось»). Сделано: проходы реестра и справочника → индексы, копии тиков → Arc, счёт котировки и поиск циклов вне рабочих потоков (`dex.quote_cpu_max`), неподвижные монеты не пересобираются, getall вне рабочих потоков. Осталось: (1) математика котировки DEX на тике — ~2–3 мс на котировку (release), до 2 котировок DEX на направление на каждый пересчёт (`core/prices.rs` for_direction: по потолку и по стакану) — главный потребитель CPU, решение по логике тика — Тиграну (docs/TASKS.md); (2) граф перестраивается на каждую версию реестра (`pools/graph.rs` build, O(N)) — инкрементальная перестройка по изменившимся парам; (3) стаканы MEXC — три копии уровней на кадр (`venues/mexc/ws_conn.rs` session: `last.insert`, `books.put(book.clone())`, событие); (4) тик стакана ядра — копии ключей и Asset на кадр (`core/market_state.rs` apply, `core/tick.rs` schedule `coin_ticks.send`); (5) `graph_route::merge` пишет Warn в журнал на каждую котировку без графа (`quote/graph_route.rs` merge); (6) размеры файлов > 200: pools/depth_book.rs (207), mexc/wallet.rs (206), mexc/mod.rs (228), quote/cycle_scan.rs (225), core/dirctx.rs (207), core/transfers.rs (217), core/profit.rs (295), core/calc_report.rs (216), kyberswap/rest.rs (265).
- **Решение 125 — размеры файлов.** Окна добавили строки в файлы, которые уже были у предела 200 строк:
  gateway/contract.rs (417), venues/bingx/mod.rs (216), venues/mexc/mod.rs (223), core/inner.rs (227),
  storage/episode_files.rs (223), core/candidates.rs (209; после решения 129 — 231), core/ledger.rs (212). Разнести при следующей правке этих файлов
  (контракт — по группам функций: биржа, DEX, окна).
- **Решение 125 — живые записи ленты сделок.** Тесты разбора сделок BingX @trade и MEXC aggre.deals на живых кадрах —
  #[ignore] до записи (tests/README.md).

- **«122 X (проба через исполнитель)» — налог покупки через StableSwap-шаг исполнителя.** Налоги монеты берутся по
  первому удачному пулу (V2 впереди); покупка через исполнитель шагом CURVE / STABLE_U256 / SADDLE идёт пул →
  исполнитель → кошелёк (два перевода), а котировка применяет налог покупки, измеренный на пути с одним переводом.
  Проверки «налог на выходе через StableSwap» в quote/graph_executor.rs нет (есть только «налог на входе»). Защита в
  сделке — проба `execute` с ценой против req.worst_price и `minOut`. Решить: сигнал для монеты с налогом покупки, как
  для налога на входе. Также check/probe.rs — 230+ строк (было 260), разнести при следующей правке.
- **Решение 122 F — DEX без исполнения через исполнитель.** KyberSwap Elastic остаётся неисполнимым (только сигнал):
  его swap(recipient, swapQty, isToken0, limitSqrtP, data) не совпадает с шагом V3 исполнителя — вида шага нет.
  Решение 122 F, 3: PancakeSwap Stable, Lista Stable, Nerve подключены как сигнал — у ArbExecutor нет вида шага
  `exchange(uint256,uint256,uint256,uint256)` (PancakeSwap, Lista; payable, без возвращаемого значения) и
  `swap(uint8,uint8,uint256,uint256,uint256 deadline)` (Saddle/Nerve) — контракт не менялся, нужен новый вид шага.
  WooFi — только квотер пула: развёрнутая реализация WooPPV2 и новый оракул не верифицированы, своей математики нет;
  ребро графа — по лесенке ответов квотера query (решение «122 (часть F, 3, лесенка)»; между точками — интерполяция
  средней цены, выше последней точки — неисполнимо); шаг WOO — после
  сухой сделки. Подписка на NewStableSwapPair фабрик PancakeSwap/Lista — нет (пулы — из справочника и поиска по реестру).
  Множитель точности Saddle берётся по decimals()
  монеты (у пула он из параметра конструктора, геттера нет) — расхождение поймает сверка с calculateSwap.
  StableSwap: во время плавной смены A (RampA) A() меняется каждым блоком без
  события — расхождение ловит только сверка с квотером и перечитывание перед сделкой; пул из N монет в реестре — одна
  пара (первая взятая).

- **Решение «122 (Infinity Stable)».** Сверка — CLQuoter (основной) и get_dy крючка (дополнительный; его считает views-контракт фабрики, код не проверен); глубина в полосе — по шаблону A_PRECISION (округление D_P иное, чем у NG; котировка — строго NG). Пулы Infinity Stable перечитываются каждым блоком (вид монеты не читается). Пауза CLPoolManager (whenNotPaused) в состоянии не читается — как у Infinity CL. Тест математики против get_dy — #[ignore] до записи (tests/README.md).

- 🔴 **ПРИОРИТЕТ 1 (Тигран, 2026-11-02). Дешёвый возврат USDT BingX → кошелёк BSC через другую сеть и мост.** Сейчас направление DEX → BingX почти не торгуется: в режиме `usdt_return.mode=per_trade` в каждую сделку закладывается вывод USDT с BingX в BEP20 — **0.15 USDT**, а плановая прибыль сигналов на бюджете 44 — 0.02–0.05. Идея Тиграна: выводить с BingX в сети Aptos (комиссия **0.01 USDT** по `config/getall`) и мостом (Stargate или другим) переводить в BSC. Разбор и план проверки — `docs/notes/usdt-return-bridge.md`. Коротко: протокол Stargate только EVM, путь «нативный USDT Aptos → Binance-Peg USDT BSC» официально не подтверждён; мост выгоднее прямого BEP20 только на небольших суммах (оценка < ~150 USDT); альтернатива без новой сети — `usdt_return.mode=separate`. Перед кодом: живые котировки моста и официальные доки, решение — Тиграну.

- ✅ **Сделано решением 121, часть B4 (ветка own-node/zone-b, не слита):** отказ верификации — только если свежая средняя цена хуже границы сделки ядра `req.worst_price` (покупка — дороже, продажа — дешевле); `verify.max_price_drift_pct` — грубый предохранитель в обе стороны (советуемое 10–20 %, задать в интерфейсе). Открыто (на решение Тиграна): проверка именно свежей прибыли против порога ядра — сейчас граница = худшая цена плана (допуск `exec.dex_slippage_pct`). Было: ⚠️ **ОЧЕНЬ ВАЖНО И ОЧЕНЬ СРОЧНО (решение Тиграна, 2026-11-01). Предторговая верификация: решать по СВЕЖЕЙ ПРИБЫЛИ, а не по фиксированному дрейфу** (решение 83, `verify.max_price_drift_pct`). Сейчас `quote/verify_decide.rs` уже пересчитывает маршрут на свежих состояниях пулов и получает `fresh_quote` (в нём настоящая свежая прибыль), НО решает строкой `if drift > verify.max_price_drift_pct → Reject` — выбрасывает сделку по фиксированной погрешности цены, НЕ глядя, выгодна ли она на новой цене. Фиксированный % по сути бессмыслен: для одного спреда и 5% дрейфа норм, для другого и 0.1% много — решать должна ВЫГОДА, а не погрешность. Надо: пересчитать прибыль на свежей цене (`fresh_quote` уже посчитан!) и пропускать сделку, если она всё ещё выше порога прибыли (ядро `precheck`/profit), иначе отменять. Порог дрейфа оставить разве что грубым sanity-предохранителем от явно битых данных (напр. 10–20%), а не как основной гейт. Пока временно `verify.max_price_drift_pct=0.5`. **Решение 381 (17.11):** после срыва покупки DEX «цена ушла» до отправки план считается заново на свежей цене с порогами ядра (exec.dex_buy_replan_max) — прибыль на свежей цене проверяется пересчётом; граница сделки по порогу прибыли — на решение Тиграна (docs/TASKS.md).
- ✅ **Сделано решением 124 (ветка router/c, не слита):** вход цикла подбирается общим поиском объёма ядра `core/size_search.rs` с кривой цикла (`core/cycle_curve.rs` → `Venue::dex_cycle_quote`), потолок — money.* и баланс базовой монеты кошелька, шаг `dex_dex.size_step_base`; `dex_dex.probe_input_base` — только для сигнала. Было: ⚠️ **ОЧЕНЬ ВАЖНО И ОЧЕНЬ СРОЧНО (решение Тиграна, 2026-11-01). DEX ↔ DEX: ДИНАМИЧЕСКИЙ подбор размера цикла** (решения 85/86). Сейчас цикл исполняется на ФИКСИРОВАННОМ `dex_dex.probe_input_base` (`quote/cycle_scan.rs`). Должно быть как у BingX ↔ DEX: подбирать объём с МАКСИМАЛЬНОЙ чистой прибылью в USDT в пределах потолка/баланса кошелька (решение 66). **Логика уже есть — переиспользовать `core/size.rs`** (кривая «прибыль от объёма», границы, тернарный поиск пика): подставить расчёт цикла (`quote/cycle.rs`) как `Curve` в оптимизатор, потолок = баланс базовой монеты кошелька минус газ/резерв. `dex_dex.probe_input_base` оставить только для показа сигнала, не для исполнения. Механизм размера ОБЯЗАН быть таким же динамическим, как в BingX ↔ DEX.
- **Своп KyberSwap и «остановленная» очередь кошелька** (решение 112): свопы KyberSwap теперь стоят в общей очереди кошелька bsc_dex и берут nonce не меньше запомненного, но состояние очереди «остановлена до ручной проверки» и удержание до занятия nonce (трекер bsc_dex) у них не проверяются — своп ждёт свою квитанцию сам до kyber.receipt_timeout_ms. Довести до полного трекера bsc_dex (отмена, «вытеснена») — отдельной задачей.
- **DEX ↔ DEX после решения 124 (техдолг):** (1) бандл-форма цикла передаёт промежуточную монету по гарантированному минимуму — граница minOut на последнем хопе почти всегда не проходит, циклы фактически торгуются только через исполнитель ArbExecutor; (2) газ разрешений Permit2 бандла в подборе размера не оценивается (его ловит проверка чистой прибыли по симуляции); (3) газ отправленного approve исполнителю, если после него цикл стал невыгоден, в учёт сделки не попадает (сделки не было); (4) нет теста ядра на всю цепочку план → сделка с настоящей площадкой (нужен записанный ответ eth_gasPrice / квитанция — tests/README.md); (5) цикл через токен с налогом не строится — вопрос Тиграну.
- **DEX ↔ DEX: свой контракт-исполнитель цикла** (решение 86, вариант 3) — ✅ по сути сделан решением 124: цикл одной транзакцией через ArbExecutor (решение 122 X), без пыли; бандл остаётся запасной формой. — убирает пыль межроутерной передачи: контракт крутит весь цикл атомарно с реальными суммами (без Permit2-фикс-минимума на каждом переходе), чище и дешевле по газу. Сейчас (v1, вариант 2) межроутерный переход идёт по гарантированному минимуму предыдущего хопа, избыток остаётся пылью в промежуточной монете. Отложено.
- **DEX ↔ DEX: подметание пыли** (решение 86) — оставшиеся промежуточные монеты межроутерных переходов (exec/cycle_bundle.rs `dust_tokens`) сейчас только фиксируются/отображаются (учёт); отдельная сделка-подметание в базовую монету — маленький follow-up. Отложено.
- **DEX ↔ DEX: форк-хоп в цикле** (решения 82, 86) — хоп через свой classic V2 Router форка (Biswap и др.) в бандле цикла v1 не собирается (только Universal Router); добавить по образцу BundleTx::ForkSwap ноги DEX (фаза 80.9b). Отложено.
- **Решение 122 (Maverick V2) — расчёт по порту, не по официальному исходнику.** Математика обмена — порт команды
  Maverick (paraswap-dex-lib), Solidity-исходник пула закрыт: держится сверкой с MaverickV2Quoter перед каждой сделкой и
  тестом на записанных ответах (пока #[ignore] — нет записи). Появится официальный исходник (v2-amm) — сверить порт
  построчно. Исполнитель зовёт пул без предела тика (int32 max/min, контракт не меняли) — при нехватке ликвидности пул
  крутит цикл до нехватки газа; защищают сверка и симуляция. Перенос бинов и TWAP не моделируются — пул перечитывается
  по каждому событию (больше вызовов узла на активных пулах). Поиск по фабрике — lookup(…, 0, N): поведение при N больше
  числа пулов не подтверждено.

- **Ревью 122: проверка списков при сохранении.** `dex.cl_tick_spacings` (int24) и `dex.v3_fee_tiers` (uint24)
  проверяются только при использовании (поиск по фабрике пишет точную причину и не ищет), а не при сохранении в
  интерфейсе: у вида настройки «список текста» в каталоге нет проверки элементов. Сделать — проверку элементов списка
  в settings/value.rs (новый вид ограничения) и сообщение в интерфейсе при вводе.
- **Ревью 122: виды WOO / MAVERICK / ELASTIC контракта** не используются ботом (семейств нет; ELASTIC — никогда, пулы
  закрыты). Включать WOO / MAVERICK — только после сухой сделки по каждому виду.
- ✅ **Сделано («122 X (стейблсвапы)»): стейблсвапы не-Curve в исполнителе** — PancakeSwap Stable Swap и Lista (вид
  STABLE_U256), Nerve (вид SADDLE). Осталось: сухая сделка «через исполнитель» по каждому (docs/OWN_NODE_SWITCH.md,
  шаг 5). Нативный BNB в этих пулах (0xEeee…) — сделано («122 X (нативный BNB в стейблсвапах)», вид STABLE_U256_BNB);
  осталось: сухая сделка «через исполнитель» через Lista slisBNB/BNB в обе стороны, проба пула через исполнитель
  (газ шага с withdraw/deposit), независимое ревью безопасности контракта.
- **Свой узел, зона C (решение 121) — ограничения:** (1) поиск по фабрикам — только V2/форк V2 (`getPair`) и V3
  (`getPool` по `dex.v3_fee_tiers`); DODO (`getDODOPoolBidirection`), Solidly (`getPool(a,b,stable)`), Algebra
  (`poolByPair`), Slipstream (`getPool(a,b,int24)`), Elastic — не ищутся по фабрике (вызовы проверки уже есть в
  `pools/verify_*`); (2) сухая сделка: продажа BNB — только на сумму не больше BNB кошелька (BNB в симуляции не
  пополняется); вход пополняется переводом от пула V2/V3 вне маршрута — нет такого пула — проверка на сумме ≤ баланса;
  выход BNB (покупка BNB) мерится только при заданном `dex.multicall3`; (3) количество монет под сумму USDT подбирается
  спот-ценой пулов «монета — USDT» (или через промежуточную) — монета без таких прочитанных пулов не проверяется.
