# tests — тесты

Моков нет. Хранилище проверяется на настоящей SQLite во временной папке, правила и аргументы — чистыми функциями.
Тесты адаптеров — только на реально записанных сообщениях площадок (появятся на этапе 2).
Новые виды значений и области настроек (выбор, адрес, верхняя граница, DEX, канал) — tests/settings_dex_scopes.rs.
Кнопки Telegram (callback_data, клавиатура, команда ядру, «только свой чат») — tests/notify_buttons.rs, чистые функции; записанных ответов getUpdates пока нет.
Дубль «Ручного управления» в Telegram (решение 156: кнопки записи = кнопки экрана по каждому виду записи, повтор каждые N, прекращение после действия, второе «Да» — без повтора) — tests/notify_mirror.rs; команды и меню бота (решение 157: /cmd@бот, /help, доступность Старт/Пауза/Стоп, «Продано вручную» и команды плашки из Telegram на настоящем ядре) — tests/notify_menu.rs. Записанных ответов Telegram нет (нужен токен бота).
Человеческие тексты алертов (решение 164: деньги и проценты — 2 знака, количество монеты, причина для известных кодов MEXC/Gate по живым ответам 04.11 через функции адаптеров, «Ответ биржи» для неизвестных, формат закрытия сделки, заголовок у каждого события) — tests/notify_human.rs.
Ядро на записанных стаканах BingX (состояние рынка, грубый спред, чёрный список без проверок) — tests/core_market_signal.rs.
Ядро целиком на настоящей SQLite (режим «Стоп» после запуска, торговля без money.*/risk.* не включается, Telegram только из своего чата, сделка после перезапуска) — tests/core_ledger_control.rs.
Итог дня и лимит дня (решения 152, 153) — tests/core_daily_limit.rs: живой случай 20261104173851521BABYDOGE (MEXC, отказ вывода, «Продать сейчас»): оценка позиции по рынку −0.212 вместо −45.036, лимит не срабатывает, факт продажи в ту же сделку; без цены — худший случай с пометкой и стоп, после факта «лимит сейчас не превышен», режим остаётся «Стоп»; «Сбросить лимит дня» — итог и сделки не меняются, лимит после отметки, переживает перезапуск, вчерашняя отметка не действует.
Отправка DEX без сети (send.max_wait_blocks ≥ send.bundle_max_blocks, цена газа отмены, обмен без минимума не собирается, поступление и обмен того же токена не совпадают) — tests/bsc_dex_send.rs, настоящая SQLite.
Порядок событий пула DEX в одном блоке по logIndex — tests/bsc_dex_events.rs.
Путь через промежуточную монету (покупка и продажа через WBNB, затраты по шагам, шаги исполнения) — tests/bsc_dex_via.rs; резервы пар заданы числами (записанных пулов BSC нет).
Очередь кошелька DEX после перезапуска (SQLite): «ждёт итога» → «остановлена до ручной проверки», снятие — tests/bsc_dex_queue.rs.
Поиск пулов DEX без ручного блока: блок развёртывания менеджера и история пулов V4 в SQLite (переживают перезапуск, по адресу менеджера), события создания только своих монет, выбор пула V4 по встречному токену — tests/bsc_dex_discovery.rs.
Ядро при «исход неизвестен» (OutcomeUnknown): без повтора при любых правилах, пауза площадки один раз; код ошибки доходит до исполнителя — tests/core_outcome_unknown.rs. Решение 154 (сделка ASTER 04.11): продажа, которую не принял ни один канал и которой нет у своего узла, повторяется с паузой stuck.reject_retry_pause_ms через режим застрявшей монеты; исчерпан stuck.unsent_retries_max — журнал Error и алерт, строка отказа в хронологии одна; где транзакция по eth_getTransactionByHash — tests/core_sell_unsent.rs. Ожидаемые поступления DEX — tests/bsc_dex_inflows.rs.
Исправления по живому запуску DEX — tests/bsc_dex_live_fixes.rs: откаты квотера «не хватает ликвидности» на записанных ответах узла BSC из журнала, «Перепроверить» одной монеты кладёт в очереди только её, пустой пул, конкретная причина невозможного расчёта V3, темп узла, стартовая загрузка и «одна запись на причину» (состояния пулов — синтетика). Темп узла и сигналы подписки — tests/bsc_dex_live_node.rs. Сводка первого прохода в ядре — tests/core_first_pass.rs.
Подписка DEX только на монеты с сетью BSC и торгуемой спот-парой у биржи (SpotPairs), запуск без подписки «на всё», без дёрганья при «недостоверно» — tests/core_dex_subscription.rs на живых данных 2026-10-24 (tests/data/live_2026-10-24: журнал BingX «нет торгуемой пары», пулы из сырого журнала узла, адреса токенов подтверждены CREATE2 PancakeSwap V3).
Подписка площадки после сбоя (живой случай 2026-10-24: «Не задана настройка bsc.node_http_rps» 215 раз, после задания — без повтора) — tests/core_subscribe_retry.rs: одна запись на причину, без второй подписки, пока идёт первая, повтор по изменению любой настройки.
Номер блока опросом вместо newHeads (решение 57) — tests/bsc_dex_heads_poll.rs на реальных номерах блоков из сырого журнала: новые номера — слушателям, тот же или меньший — нет; цена до/после по таблице NodeReal.
Возраст головы по времени блока (решение 121, A2) — tests/bsc_dex_head_age.rs на живых ответах eth_getBlockByNumber 2026-10-25: время — секунды Unix, кэш не откатывается на старый блок.
Адреса вида Url (решение 121, A1): https/wss — любой хост, http/ws — только адрес-петля — tests/settings_url_loopback.rs.
Первичный фильтр BingX по справочнику пар на живых ответах 2026-10-24 (tests/data/bingx_2026-10-24: куски common/symbols и config/getall) — tests/bingx_pairs_filter.rs: пары status 1 с закрытым API (9 монет) не торгуются, «пара не торгуется (status N)», BEP20 без адреса — NoContract, ввод и вывод по отдельности, изменение справочника пар видно, устаревший справочник — PairsStale; предпроверка стороны сделки по флагам API и строгий разбор apiStateBuy/Sell — tests/bingx_pairs_precheck.rs.
Справочник сетей BingX (живой случай 2026-10-24 12:42, 100 монет «закрыт» разом; причина не установлена) — tests/bingx_transfer_status.rs: флаг сети только boolean, массовая смена — одна сводка (venue.status_bulk_min).
Подписки пулов DEX на живых данных 2026-10-24 (tests/data/live_2026-10-24/bsc_dex_subs.json) — tests/bsc_dex_subs_live.rs: 1968 подключений истории V4 из журнала раскладываются в ≤ 13 пачек (attach::split); мелкий пул — только Mint из реального лога (Swap в фильтр не входит); topic0 V4 из реальных логов.
Multicall3 и резервный узел — tests/bsc_dex_multicall_failover.rs: селектор aggregate3 из официального байткода, ответ по вызовам (реальный пул LIGHT/USDT и записанный откат QuoterV2), переключение только на связь/таймаут/5xx/429.
Справочник пулов от индексатора (решение 58) — tests/bsc_dex_directory.rs на живых ответах GeckoTerminal 2026-10-24 (tests/data/indexers_2026-10-24): разбор пулов пяти монет (PoolId у V4/Infinity), список DEX по двум страницам, HTTP 400 на третьей — «страниц больше нет», коды 401/429/5xx, отбор пар с USDT и промежуточной монетой.
Пулы из справочника → пулы подписки (решения 58–60) — tests/bsc_dex_index.rs (форки не в сделках, смена вида на лету, умный роутер, сбой узла — «не проверен», «кто меняет», ABI V3 по lmPool(), очистка устаревших настроек): таблица сопоставления (не подключена, не поддерживается, выключена, нет адреса), итог по пулам CAKE из записи GeckoTerminal, решения проверки на узле по ответам — чужая фабрика, пул-подделка, нет в реестре фабрики, комиссия пары Biswap, ключ Infinity у менеджера (ответов узла в записи нет — значения из записи индексатора и официальных адресов).
Проверка пулов на ЖИВЫХ ответах узла BSC (блок 0x7b8a659) — tests/bsc_dex_node_verify.rs: все пулы PancakeSwap V2/V3 и Uniswap V3 CAKE и ETH из записи проходят (29), реестр getPair совпадает, PancakeSwap V1 / THENA V3 / Topaz — отказ по фабрике, lmPool() различает PancakeSwap и Uniswap V3, ключи Infinity и Uniswap V4 дают тот же PoolId, aggregate3 = отдельные вызовы. Второй проход: getPool фабрик V3 совпадает у всех 17 пулов, «кто меняет» — на настоящих логах обмена из квитанций (свой пул признаётся, чужой — «меняет в другом пуле»). Вызовы для записи — tests/data/node_calls_2026-10-24.json и node_calls_2_2026-10-24.json (генератор — tests/bsc_dex_node_calls.rs, вручную с `--ignored`).
Очередь узла со срочностью (решение 61) — tests/bsc_dex_node_priority.rs: освободившееся место получает сделка, потом проверка, потом фон независимо от порядка прихода; внутри класса — по приходу и с темпом; отменённый ждущий не держит очередь; класс задачи — из scope. Откат «срок транзакции вышел» узнаётся по селектору 0x5bf6f916 из живого журнала Тиграна 2026-10-24 (GTAI, XRP) — = TransactionDeadlinePassed() Universal Router.
Метки отбора DEX (решение 61) — tests/core_dex_subscription.rs (not_searched_and_loading_are_not_no_usdt_pool): монета без пары на бирже (BITLIGHT) DEX-присутствия не получает; монета подписки с найденными, но не прочитанными пулами (CAKE, PoolsLoading) — «пулы загружаются», а не «нет пула с USDT».
Пачечное чтение состояния пулов (решение 61, #47) — запись живых ответов узла: tests/bsc_dex_node_calls_3.rs (вручную, `--ignored`). write_node_calls_3 — точный список ступени 1 (tests/data/node_calls_3_2026-10-24.json: V2 getReserves, V3 slot0 + liquidity, Infinity CL и Uniswap V4 getSlot0 + getLiquidity для пулов CAKE и ETH, каждый отдельно и пачкой aggregate3 по dex.verify_batch); record_node_calls_3 — все три ступени (слова тиков и тики зависят от ответов) за один проход на закреплённом блоке → tests/data/indexers_2026-10-24/arb_node_responses_3_<дата>.json. Переменные: ARB_NODE_URL, ARB_TICK_WORDS (= dex.tick_words), ARB_BATCH (= dex.verify_batch). Тест разбора на этих ответах — после записи.
То же на JavaScript для записи из браузера (сети к BSC из песочницы нет, а eth_getLogs и старые блоки публичный узел не отдаёт) — tests/recorders/record_node_calls_3.js: вставить в консоль браузера, всё за один проход на одном блоке (bsc-dataseed.bnbchain.org, TICK_WORDS = 2, BATCH = 200; пулы и ступень 1 встроены из node_calls_3_2026-10-24.json), итог {about, block, results} — в window.__rec. Кодирование сверено с Rust-рекордером: пачка ступени 1 совпадает байт в байт с node_calls_3_2026-10-24.json, вызовы ступеней 2–3 и их пачки — с кодировщиками бота (sol!, load_cl::set_ticks) на отрицательных тиках и битах 0/7/255.
Пачечное чтение на живых ответах (решение 61, #47) — tests/bsc_dex_batch_load.rs: запись 2026-10-25, блок 0x7b936ae (arb_node_responses_3_2026-10-25.json, 1060 вызовов, рекордер на JavaScript из браузера): вызовы всех трёх ступеней, которые строит бот (головы; слова по тику из ответа; тики CL по битам из ответа), — ровно записанные; пачки aggregate3 (по 200) — запрос = кодировщик бота, ответы по вызовам = отдельные eth_call (1052 вызова); карты тиков разбираются. Откат одного вызова в пачке задевает только его пул — на настоящих откатах записи 2026-10-24 (fee/tickSpacing/lmPool у пар V2 и Uniswap V3).
BNB ↔ WBNB (решение 62): справочник ядра — tests/core_native.rs (настройки на настоящей SQLite; записи BNB из настоящих ответов BingX config/getall — сопоставляется только BEP20, закрытые XTERIOB и Opbnb не трогаются); план сделки DEX — tests/bsc_dex_native.rs (продажа [WRAP_ETH, обмен из роутера, BALANCE_CHECK_ERC20] с value, покупка [обмен роутеру, UNWRAP_WETH], отказ для V4 через промежуточную). Вызовы для записи живых ответов (eth_simulateV1 с WRAP_ETH/UNWRAP_WETH, WETH() роутера, getPair) — tests/data/native_calls_2026-10-24.json, собраны кодом сделки; тест сверяет файл с кодом (перезапись — `--ignored`). Живые ответы узла на эти вызовы (tests/data/native_2026-10-25, блок 0x7b94144) — тот же тест: WETH() = WBNB, getPair, «кто меняет», газ симуляции, факт по Deposit/Withdrawal WBNB, нативный перевод без событий.
После ревью #50: тесты нативной монеты разнесены — tests/native_common (общее: окружение, план, списки вызовов), tests/bsc_dex_native.rs (план V2 и V3 через UR PancakeSwap и Uniswap, списки вызовов совпадают с кодом), tests/bsc_dex_native_live.rs (живые ответы: moved(), «кто меняет», газ, статус нативного перевода native_status::judge); второй список вызовов (V3, UR Uniswap) — tests/data/native_calls_2_2026-10-25.json. Газ в BNB у сделки с BNB — не в количестве (core_native.rs, gas_never_in_coin_qty). Статус нативного перевода — на настоящем выводе BNB с BingX (tests/data/native_2026-10-25/arb_bingx_bnb_withdraw_2026-10-25.json): прямой перевод, сумма зачисления = value транзакции. BNB через V3 (UR PancakeSwap и Uniswap) на живых ответах и отсутствие Infinity CL WBNB/USDT у индексатора — tests/bsc_dex_native_v3_live.rs; V4 / Infinity для BNB — отказ NoRoute (tests/bsc_dex_native.rs).
HTTP с двумя сроками (решение 63) — tests/http_body_read.rs: записанный ответ узла на пачку aggregate3 (~1 МБ, запись 2026-10-25) отдаётся порциями медленно — общий срок reqwest его обрывает, два срока дочитывают байт в байт; замолчавшее тело — «ответ не дочитан, получено N байт», закрытый порт — «нет связи». Монета ждёт только своих пулов, брошенные пулы возвращаются при изменении настроек — tests/bsc_dex_coin_start.rs (пулы CAKE и ETH из записи GeckoTerminal).
Коды сбоев HTTP и повтор BingX на резервный адрес (решение 63) — tests/http_fail_codes.rs: Unreachable — только «соединение не установлено», запрос дошёл без ответа — Timeout; резервный адрес — только если не дошёл или срок у справочников (Op::Other). Решения цикла налогов — tests/bsc_dex_tax_wait.rs: монета с прочитанными своими пулами проверяется сразу (и без сигнала), сводку первого прохода держат монеты с читающимися пулами (reading_coin_holds_first_pass_summary), не держат монеты с застрявшими пулами («пулы не читаются (ждут настройку): N» — отдельно, один раз). tests/bsc_dex_coin_start.rs: флаг «застрял» у пула (stuck_only), имя незаданной настройки — поле GatewayError.setting настоящей ошибки, возврат брошенных только по своей настройке. tests/outcome_unknown_sources.rs: ордер и вывод BingX при любом сбое, кроме Connect (включая TooBig, NotText), — OutcomeUnknown.
«Результат неизвестен» (решение 64) — tests/outcome_unknown_sources.rs: ордер или вывод BingX без ответа целиком (и HTTP 504) — OutcomeUnknown, не связались — Unreachable, справочники — как было; повтор запрещён только для OutcomeUnknown (retry_allowed, no_retry в справочнике формы правил).
Подбор объёма (решение 66) — tests/core_size_optimum.rs, tests/core_size_limits.rs; записи CAKE — tests/size_common:
стакан BingX CAKE-USDT@depth20 (tests/data/bingx), правила пары (stepSize, minQty, maxQty, minNotional, maxNotional) и сеть BEP20
(withdrawFee, минимумы, withdrawPrecision) — tests/data/bingx_2026-10-24, пул CAKE/USDT PancakeSwap V2 (резервы — ответы узла 3);
обмен считает адаптер DEX (pool_quote::swap), пределы ордера, шаг, точность вывода и вид отказа — функции ядра. Проверки:
списание по худшей цене ≤ бюджета на любом бюджете в обе стороны; оптимум не хуже перебора; случай «бюджет / лучшая цена» со
списанием чуть больше бюджета (живой отказ WFI); minQty 2.672 — наименьший объём; объём больше резерва пула — граница сверху;
size.max_evals; пыль; точность вывода. Полный costs::evaluate на записях не собирается: нет записей ставки taker, цены газа,
стакана BNB и σ. Записей WFI и DEOD нет — кейс повторён на CAKE.

Проверки пулов на старте (решение 65) — tests/bsc_dex_check_start.rs: пул CAKE/ETH из записи GeckoTerminal проверяется, как только прочитан (а не через dex.quoter_check_s); недостоверный и проверенный — не берутся; сводка первого прохода — когда ни один пул не читается (застрявшие не держат) и все прочитанные проверены; пул, недостоверный во время прохода, не отмечается проверенным и проверяется сразу после чтения (invalid_during_pass_is_checked_as_soon_as_read).

Налоги служебной монеты USDT (решение 67) — tests/bsc_dex_service_tax.rs: список вызова eth_simulateV1 для записи (tests/data/service_tax_2026-10-25/calls.json) сверяется с кодом service_tax::calls; разбор живого ответа узла (responses.json, блок 0x7bb0bc5) — налог USDT 0 по всем трём переводам, газ перевода 51603.

События пулов из квитанций блоков (решение 68) — tests/bsc_dex_receipts.rs и tests/bsc_dex_receipts_chain.rs на живой записи eth_getBlockReceipts публичного узла BNB Chain (tests/data/receipts_2026-10-25/, рекордер tests/recorders/record_receipts.js): логи фильтруются теми же фильтрами, что подписка WebSocket (V2 и V3 в окне), формат — как у подписки; цепочка по parentHash, реорг — откат с removed: true; событие создания пула находится фильтром новых пулов; реорг — откат настоящих логов наших пулов с removed: true, только подтверждённый вторым ответом; квитанции полные по хешам транзакций заголовка; логи блока и номер блока — одним действием; Infinity CL и Uniswap V4 — на записи responses_3.json.

Конвейер ленты квитанций (решение 69) — tests/bsc_dex_receipts_pipe.rs: заголовок и квитанции блока одним JSON-RPC batch — разбор живого ответа публичного узла (tests/data/receipts_batch_2026-10-25/, рекордер tests/recorders/record_receipts_batch.js); конвейер на блоках записи отдаёт их строго по порядку, не пришедший блок запрашивается снова, следующие ждут и повторно не запрашиваются.

Проба крючка без ликвидности (решение 70) — tests/bsc_dex_live_fixes.rs (hook_probe_not_enough_liquidity_names_pool_id): настоящий откат квотера UnexpectedRevertBytes(NotEnoughLiquidity) из журнала — PoolId извлекается, причина «в пуле не хватает ликвидности для пробы». Batch ленты квитанций: ответы в обратном порядке и ошибка одного вызова внутри пачки — tests/bsc_dex_receipts_pipe.rs.

Тяжёлый блок и отставание ленты, чёрный список при сбросе (решение 71) — tests/bsc_dex_receipts_pipe.rs (lag_names_the_block_the_pipe_stands_on), tests/core_first_pass.rs (reset_of_pools_is_not_a_blacklist_event: временное «ещё не проверена» и возврат к той же причине — без записей, только настоящие вход и выход).

Догон ленты без сброса (решение 72) — tests/bsc_dex_receipts_catchup.rs: отметка отставания запрещает котировку и снимается после догона; пропуск невосполним, только если блока нет ни у одного узла (живая batch-запись, заголовок null); блоки по кругу на узлы списка, у каждого свой темп.

Ревью #62 (решение 72) — tests/bsc_dex_receipts_catchup.rs: «невосполнимо» только если блок не отдал ни один узел, у которого своя голова не ниже блока (головы ниже — повтор); снятие отметки «отстают» будит циклы проверок. Гонка повтора (первый полный ответ) — tests/bsc_dex_receipts_race.rs.

Повторное ревью #62 (решение 72) — tests/bsc_dex_receipts_reset.rs: после сброса ленты котировки запрещены, пока не закрыты все «соединения» пачек старой ленты (после их Invalidated), снятие циклы не будит; смена источника на ws снимает отметку после закрытия «соединений» пачек, обратно на receipts — отметка заново до догона; один узел — «невосполнимо» только после двух «блока нет» подряд (живая batch-запись).

Глубина пулов (решение 73) — tests/bsc_dex_depth_hysteresis.rs: гистерезис вердикта «глубокий ↔ мелкий» (не мигает у порога), тиков и слов битмапа на полосу при tickSpacing 1 / 10 / 60, «missing trie node» неархивного узла — своя причина StaleData (ответ из записи глубины 2026-10-25). Глубина в полосе на живой записи 2026-10-25 (tests/data/depth_2026-10-25/responses.json, рекордер tests/recorders/record_depth.js, у каждого пула свой блок) — tests/bsc_dex_depth_band.rs: пулы USDT монет ZKP / OPN / IOST (Infinity CL и Uniswap V4, tickSpacing от 1): полоса ±2% прочитана целиком, шире полоса — не меньше глубина, без тиков внутри полосы — совпадение с формулой одного диапазона; AAVE Uniswap V3 0x8e3ecc0b…: продажа по прочитанной карте отдаёт 94,3% USDT пула (balanceOf на том же блоке — 1915,19), остальное — за прочитанными словами и невыплаченные комиссии.

Глубина, ревью #63 (решение 73): общее разбора записи — tests/depth_common/ (cl.rs, pairs.rs); полоса шире прочитанных слов — full = false (tests/bsc_dex_depth_band.rs); V3 AAVE в полосе и пары V2 (монета токен0 и токен1) против формул — tests/bsc_dex_depth_pairs.rs; кэш с гистерезисом, журнал смен с ограничением частоты и отложенной записью, сигнал «пул готов» ровно один раз — tests/bsc_dex_depth_book.rs.

Стороны пула независимо и глубина без комиссии (решение 73, решения Тиграна 26.10) — tests/bsc_dex_depth_sides.rs: пул, глубокий только на продажу, котируется на продажу и не котируется на покупку; падение глубины покупки не убирает продажу; сторона без своей сверки не котируется; tests/bsc_dex_depth_band.rs: IOST V4 0xbccf1b3f… без комиссии — покупка и продажа 2,45 / 2,45 USDT (с комиссией было 24,54 / 2,45).

Ревью #63: сторона без итога, ставшая глубокой, сверяется одним проходом цикла по сигналу «пул готов» (tests/bsc_dex_depth_sides.rs — запускает check/quoter_cycle.rs one; квотера в тесте нет, итог стороны — «не задана настройка узла», важно, что он есть); монеты пробы продажи по цене пула — tests/bsc_dex_spot.rs. Известный пробел покрытия (решение техлида — записи не делаются): (1) сверка «сверена и котируется» с настоящим ответом квотера — тест проверяет механику цикла (сторона получает итог одним проходом по сигналу), но не сам ответ QuoterV2; (2) пул CL с ликвидностью только с одной стороны цены — coins_for проверен на всех записанных пулах, одностороннего в записи 25.10 нет (самый перекошенный IOST Infinity 0x0ca12d… — 0,0185 / 0,0025 USDT в ±2%).
Решение 74 — tests/core_dry_mode.rs (сухой режим на настоящем ядре: исполнитель отказывает первым шагом, сделок нет; режим на лету; без бюджета — причина; гонка «сухой включили во время предпроверки» — бюджет живой сделки не виртуальный, замок перед покупкой; повреждённое dry.enabled — сухой; путь очереди кандидатов в сухом режиме; пересчёт сделки перед покупкой — Live (precheck::replan); ссылки по шаблонам из настроек: пара — по площадке, пул — по DEX), tests/core_signal_max.rs (максимальный объём и прибыль без бюджета на записанных стакане и пуле CAKE, край — maxQty пары или предел поиска; ликвидность стакана в полосе; подстановка шаблонов ссылок), tests/core_episodes.rs (открытие и закрытие эпизода по последовательности вердиктов; через очередь кандидатов — файл zstd и сводка в SQLite от задачи эпизодов; уборка закрывает эпизод пропавшего направления; место кончилось — новый эпизод цел, удалён самый старый). Расчёт ядра на записях CAKE — tests/size_common/calc.rs.

Настройка на лету (решение 75) — tests/bsc_dex_config_wake.rs: без dex.depth_band_pct отбор пулов пробы налогов на живом пуле AAVE V3 — «не задана настройка» (Why::Config); изменение настроек сразу выдаёт монету на проверку (без полного прохода), после задания отбор проходит; цикл без интервала ждёт его, а не останавливается.

Ревью #65 (решение 75) — tests/bsc_dex_config_wake2.rs: смена порога глубины пересчитывает вердикт без событий пула и будит монету Why::Shallow (живой пул AAVE V3); ошибки узлов ленты — только хост (ответ «missing trie node» из записи 25.10); «догнала» — по окну; общий механизм ожидания (BingX) — одна запись «ждёт», одна «снова работает».

Повторное ревью #65 — tests/bsc_dex_config_wake.rs: 30 изменений посторонних настроек — пачка схлопнута, ни одной перепроверки; изменение нужной — одна; tests/bsc_dex_config_wake3.rs: имя настройки у стороны сверки — структурно, перепроверка только по нему; Why::Lag — выдаётся, как только отметка снята; пересчёт глубины замораживает пачку на время чтения (пулы недостоверны, котировка отказывает), затем снова достоверна. Ревью #65 (ddb140d): пул, не прочитанный пересчётом глубины, недостоверен — котировка отказывает; имя незаданной настройки у сверки — из ошибки (check.probe_usdt, затем dex.quoter_max_diff_pct — до обращения к узлу).

Решение 76 (зачисление по событию): tests/bsc_dex_wallet_events.rs — на живой записи квитанций (tests/data/receipts_2026-10-25/responses.json, блок 129702236) фильтры «Transfer на кошелёк / с кошелька» выбирают ровно логи с адресом в from или to (посчитанные независимо); перевод 58,69 USDT (транзакция 0x3263…0835) для получателя — поступление не своей транзакцией (Deposit), для отправителя — своё списание; выбор ожидания по сумме (одно / ни одного / несколько). tests/bingx_account_update.rs — кадр ACCOUNT_UPDATE из примера официальной документации (INIT — снимок, изменений нет), причины m из той же страницы; ожидаемый приход вывода биржи «сумма − комиссия» на настоящем выводе BNB с BingX (0,01998 − 0,0005 = 0,01948 = value транзакции). Известный пробел покрытия до записи: (1) кадры ACCOUNT_UPDATE с m = DEPOSIT / WITHDRAW / ORDER с живого потока — запись делает Тигран (включает venue.balance_events [bingx], депозит токена с кошелька на BingX и вывод), тесты — по сырым файлам data/raw/bingx («account») и ответам deposit/hisrec; (2) баланс BNB по блокам (wallet/events_native.rs) — состояние старых блоков публичный узел не отдаёт (не архивный), запись — вместе с первым живым переводом BNB. Связка «ожидание → событие → закрытие» в настоящем ядре (tests/core_credit_events.rs, ревью #66): хеш отправителя совпал — закрыто; хеша ещё нет — только пробуждение; чужой хеш — не закрывает (снимается при хеше отправителя, после него — не прикрепляется); закрытие выключено — событие сопоставлено, но не закрывает.

Решение 151 (зачисление по блокчейну, tests/core_credit_chain.rs): на той же записи блока 129702236 в настоящем ядре — приход 58,69 USDT (0x3263…0835) на кошелёк сделки засчитан без хеша биржи; другая сумма (первый перевод USDT блока), другой токен (первый не-USDT перевод блока), другой кошелёк, приход раньше запроса вывода и transfer.credit_by_chain = нет — не засчитаны; второй ожидаемый приход того же токена на тот же кошелёк — не засчитан. Фоновая сверка хеша биржи (transfer_chain.rs) записанным ответом withdrawal_status не покрыта — запись нужна с живой сделки.

Решение 77 (устойчивость ленты): tests/bsc_dex_receipts_resilience.rs — два условия восстановления «пропуск + пересинхрон» на настоящей причине живого бота (блок 129907931, «квитанции больше предела bsc.receipts_total_ms»): пропуск по bsc.receipts_block_max_tries «жёстких» отказов подряд (мягкий отказ счётчик обнуляет, настройка 0 — выключено); пропуск по длительности отставания (bsc.receipts_stuck_reset_s), причём таймер не сбрасывается при ползущей ленте (passed/failed по разным блокам) — ровно дыра, где прежний триггер по возрасту одного блока не срабатывал, — а реальный догон ≤ max_lag его снимает. Конвейер на записанных блоках и гонка повтора — tests/bsc_dex_receipts_pipe.rs; «сброс» и его снятие по закрытию «соединений» — tests/bsc_dex_receipts_reset.rs. Пропуск целиком до головы с пересинхроном пулов без разрыва подписок (depth_sync::resync + release_if_resynced) проверяется механикой lag-триггеров (данных не требует) и не гоняется end-to-end: для этого нужен живой узел (сеть к BSC из песочницы закрыта).

Решение 79 (ws-подписка logs как источник): tests/bsc_dex_ws_source.rs на живых записях публичного узла wss://bsc-rpc.publicnode.com (tests/data/ws_logs_2026-10-26, рекордер record_ws_logs.js): кадр eth_subscription = лог квитанции, и фильтр адаптера by_address (V2/V3) доставляет настоящий записанный кадр (node/log_filter.rs); by_pool_id кладёт наши PoolId в topics[1] у менеджера — лог с нашим PoolId проходит, с чужим нет (CL/V4, вариант (а)); по записи пределов — один фильтр принял 5000 адресов и 5000 PoolId/topics[1] на обоих менеджерах, 10 соединений на IP держатся (дробить ~2850 пулов не нужно). Селекторы событий — из ABI пути квитанций, запись подтверждает доставку и формат. Пробел покрытия: removed:true (реоргов за окно записи не было) — как у depth, на длинной записи; полный прогон источника — нужен живой узел (сеть к BSC из песочницы закрыта).
- `mexc_real_data.rs` — справочник пар MEXC на живом exchangeInfo 2026-11-02 (`data/mexc_2026-11-02`), решение 107.
- `core_usdt_return_retry.rs`, `core_usdt_return_manual.rs` (общее — `usdt_return_common/`) — решение 163 (живая сделка 20261104225504576DOGE, отказ MEXC 16021 разбирает настоящий адаптер): какие сбои вывода безопасно повторять; повтор N раз с интервалом usdt.return_retry_s и строка «попытка N из M» на каждый; успех на k-й попытке — записи нет; все неудачны — запись «USDT не вернулись» с «Перегнать» и «Перегнано вручную», страховка одна; без настроек — без повторов, журнал один раз; «ждёт повтора» переживает перезапуск; старая not_returned из учёта — сразу на экране; «Перегнано вручную» закрывает, повторное нажатие — без действия; «Перегнать» — одна попытка; «Продано вручную» — «Перегнать» недоступна.
- `withdraw_hold.rs` — решение 150 (удержания вывода): расчёт «можно вывести = свободно − удержания в USDT» на числах живого случая 04.11 (MEXC 16021, предварительно зачислено 45 USDT); разбор MEXC deposit/hisrec со status = 9 — `#[ignore]` до приватной записи `data/mexc_deposit_hisrec_pre_success.json` на живом боте (как записать — шапка файла).
- `bybit_real_data.rs` — Bybit (решение 136) на живых публичных ответах 2026-11-04 (`data/cex_samples_2026_11_04/bybit_*`): справочник спот-пар, правила ордера CAKEUSDT, поток orderbook.50 (snapshot + delta, u подряд, локальный стакан; строгая проверка u — venue.book_seq_strict: тот же поток с выброшенным кадром delta даёт разрыв только при строгой), кадры окна (publicTrade, Book, Diff), деление на соединения. Приватные (#[ignore]) — записать на живом боте: coin/query-info (весь и ?coin=), wallet-balance UNIFIED, query-api, account/info (unifiedMarginStatus — тип счёта в «Проверить подключение»), первый IOC-ордер с кадрами order.spot/execution.spot, realtime и execution/list, депозит query-record?txID=, inter-transfer FUND → UNIFIED, вывод и query-record, query-address.
- `kucoin_real_data.rs` — адаптер KuCoin (решение 137) на живых публичных ответах 04.11 (`data/cex_samples_2026_11_04/kucoin_*`:
  /api/v2/symbols, /api/v3/currencies и /CAKE, bullet-public, level2_20 CAKE-USDT, kucoin_ws.jsonl — welcome, ack, pong,
  level2Depth50, level2) и на тестовом векторе подписи официального SDK (test_default_signer.py). `#[ignore]` до записи:
  (1) `recorded_match_decode` — `data/cex_samples_2026_11_04/kucoin_match.jsonl`, по строке кадр /market/match:<SYM>
  как пришёл (в записи 04.11 за 20 с сделок CAKE-USDT не было — писать ликвидную пару минуту); (2) `recorded_balance_frames`
  — `kucoin_account_balance.jsonl` на живом боте: кадры приватной темы /account/balance (депозит main.deposit, перевод
  main.transfer/trade.transfer, сделка trade.hold/trade.setted, вывод main.withdraw_hold/withdraw_done) — из сырого
  журнала data/raw/kucoin, соединение «account». Ещё записать на живом боте (без ключей и токенов): ответ
  /api/v1/user/api-key (permission, apiVersion), /api/v1/hf/accounts/opened, /api/ua/v2/account/mode (selfAccountMode — тип счёта Classic / UTA), ответы hf/orders/test, hf/orders/sync,
  GET /api/v1/hf/orders/{id} и /api/v1/hf/fills одной сделки, /api/v3/deposit-addresses?chain=bsc, /api/v1/deposits,
  /api/v1/withdrawals/{id}, /api/v3/accounts/universal-transfer, кадры /spotMarket/tradeOrdersV2.
- `bitget_real_data.rs` — адаптер Bitget (решение 162, Classic API v2). Без записей (идут всегда): подпись — значения
  функций официального SDK (`bitget-python-sdk-api/bitget/utils.py`: `pre_hash`, `sign`), строка запроса как
  `parse_params_to_str` SDK, правило своего clientOid (≤ 32, Best Practices), подписка пачками ≤ 4096 байт. `#[ignore]` до
  записи в `data/cex_samples_2026_11_05/` (с сервера бота; из среды разработки api.bitget.com закрыт прокси) — раздел ниже.

### Bitget — команды записи (решение 162)

Вектор подписи пересчитать функциями SDK (каталог SDK — github.com/BitgetLimited/v3-bitget-api-sdk):

```bash
python3 -I -c "import sys; sys.path.insert(0, 'bitget-python-sdk-api'); from bitget import utils; \
print(utils.sign(utils.pre_hash(1695808949356, 'GET', '/api/v2/spot/account/assets?coin=USDT', ''), 'test_api_secret'))"
# X+fN2ZKIaQNx8I+bkoc82/yTFv2yygsJ5/FLmL2+zIM=
```

Публичные (ключ не нужен; монеты CAKE и USDT; адреса и параметры — по официальной документации Classic v2):

```bash
D=tests/data/cex_samples_2026_11_05; mkdir -p $D
# Get Symbol Info — все пары и одна (CAKEUSDT)
curl -sS --compressed 'https://api.bitget.com/api/v2/spot/public/symbols' > $D/bitget_symbols.json
curl -sS --compressed 'https://api.bitget.com/api/v2/spot/public/symbols?symbol=CAKEUSDT' > $D/bitget_symbols_CAKEUSDT.json
# Get Coin Info — все монеты (замерить размер: wc -c), CAKE и USDT. Путь документации — /public/; SDK зовёт /market/coins
curl -sS --compressed 'https://api.bitget.com/api/v2/spot/public/coins' > $D/bitget_coins.json
curl -sS --compressed 'https://api.bitget.com/api/v2/spot/public/coins?coin=CAKE' > $D/bitget_coins_CAKE.json
curl -sS --compressed 'https://api.bitget.com/api/v2/spot/public/coins?coin=USDT' > $D/bitget_coins_USDT.json
curl -sS -o /dev/null -w '%{http_code}\n' 'https://api.bitget.com/api/v2/spot/market/coins?coin=CAKE'
# Имя сети BSC (поле chain у CAKE) — одной строкой в $D/bitget_bsc_network.txt (его же — в venue.bsc_network)
python3 -c "import json; d=json.load(open('$D/bitget_coins_CAKE.json'))['data'][0]; print([c['chain'] for c in d['chains']])"
# Стакан REST
curl -sS --compressed 'https://api.bitget.com/api/v2/spot/market/orderbook?symbol=CAKEUSDT&type=step0&limit=150' > $D/bitget_orderbook_CAKEUSDT.json
# WebSocket books15 (CAKEUSDT) и books5 (USDCUSDT) — по строке на кадр, «ping» раз в 30 с, ~65 секунд
( echo '{"op":"subscribe","args":[{"instType":"SPOT","channel":"books15","instId":"CAKEUSDT"},{"instType":"SPOT","channel":"books5","instId":"USDCUSDT"}]}'; \
  sleep 30; echo ping; sleep 30; echo ping ) | timeout 65 websocat -t wss://ws.bitget.com/v2/ws/public > $D/bitget_ws_books15.jsonl
```

Что проверить по записи: есть ли `contractAddress` у сети BSC CAKE (0x0E09…cE82); `withdrawFee`, `extraWithdrawFee`,
`minDepositAmount` («0» или пусто?), `withdrawMinScale`, `withdrawStep`, `needTag`; `pricePrecision` / `quantityPrecision`
CAKEUSDT и есть ли `maxLimitOrderValue`; шлёт ли books15 одинаковые снимки (тот же `seq`) без изменений рынка; порядок
уровней; ответ на подписку.

Приватные — на живом боте (без ключей в файлах; из сырого журнала data/raw/bitget, записи «rest <путь>»):
`bitget_all_trade_rate.json` (GET /api/v2/common/all-trade-rate?businessType=spot), /api/v2/spot/account/info
(authorities, ips), /api/v2/spot/account/upgrade-status (что отвечает классический счёт), /api/v2/spot/account/deduct-info,
/api/v2/spot/account/assets (регистр имени монеты), /api/v2/account/funding-assets, /api/v2/account/all-account-balance,
/api/v2/spot/wallet/deposit-address?coin=CAKE&chain=<сеть BSC> (формат поля chain), первый IOC-ордер: ответ place-order и
`bitget_order_info.json` (GET /api/v2/spot/trade/orderInfo — status cancelled или canceled, знак и вид feeDetail), депозит
и вывод в deposit-records / withdrawal-records (на какой счёт пришёл депозит; входит ли комиссия в size вывода), перевод
/api/v2/spot/wallet/transfer. Приватный WebSocket: единица времени входа (мс по тексту документации или секунды по примеру
Java и SDK) и кадры канала баланса спота (страница канала не прочитана — поток аккаунта Bitget не подключён).
- `binance_real_data.rs` — адаптер Binance (решение 160). Всегда выполняются: подпись REST и WebSocket API на
  официальных примерах документации (request-security: `c8db5682…6b71`; websocket-api/request-security: `aa1b5712…db24`).
  Остальное — `#[ignore]` до записи в `data/cex_samples_2026_11_05/` (из среды Claude `api.binance.com` закрыт прокси).

### Binance — команды записи (на сервере бота)

Публичные (без ключа):

```bash
D=tests/data/cex_samples_2026_11_05; mkdir -p $D
curl -sS 'https://api.binance.com/api/v3/exchangeInfo?symbol=CAKEUSDT' -o $D/binance_exchange_info_CAKEUSDT.json
curl -sS 'https://api.binance.com/api/v3/exchangeInfo' -o $D/binance_exchange_info.json   # весь справочник: размер и rateLimits
curl -sS 'https://api.binance.com/api/v3/depth?symbol=CAKEUSDT&limit=100' -o $D/binance_depth_CAKEUSDT_100.json
# частичный стакан через комбинированный поток (как у бота): ответ {"result":null,"id":1} и кадры {"stream","data"}
{ echo '{"method":"SUBSCRIBE","params":["cakeusdt@depth20@100ms"],"id":1}'; sleep 15; } \
  | websocat -t 'wss://stream.binance.com:9443/stream' > $D/binance_ws_depth20.jsonl
# окна: сделки и изменения стакана — сырые потоки /ws/<stream> (минуту: CAKE торгуется не каждую секунду)
timeout 60 websocat -t 'wss://stream.binance.com:9443/ws/cakeusdt@trade' > $D/binance_ws_trade.jsonl
timeout 20 websocat -t 'wss://stream.binance.com:9443/ws/cakeusdt@depth@100ms' > $D/binance_ws_diff.jsonl
```

Приватные (ключ Binance; ключ и подпись в файлы не попадают — только тела ответов):

```bash
K='<API key>'; S='<Secret key>'
bsig() { # $1 метод, $2 путь, $3 параметры без timestamp (может быть пусто)
  local q="${3:+$3&}recvWindow=5000&timestamp=$(date +%s%3N)"
  local s=$(printf '%s' "$q" | openssl dgst -sha256 -hmac "$S" | sed 's/^.* //')
  curl -sS -X "$1" -H "X-MBX-APIKEY: $K" "https://api.binance.com$2?$q&signature=$s"; }
bsig GET  /api/v3/account 'omitZeroBalances=true'          > $D/binance_account.json
bsig GET  /api/v3/account/commission 'symbol=CAKEUSDT'     > $D/binance_account_commission_CAKEUSDT.json
bsig GET  /sapi/v1/capital/config/getall ''                > $D/binance_config_getall.json
bsig GET  /sapi/v1/capital/deposit/address 'coin=CAKE&network=BSC' > $D/binance_deposit_address_CAKE.json
bsig GET  /sapi/v1/capital/deposit/hisrec ''               > $D/binance_deposit_hisrec.json
bsig GET  /sapi/v1/capital/withdraw/history ''             > $D/binance_withdraw_history.json   # вес 18000 — один раз
bsig GET  /sapi/v1/account/apiRestrictions ''              > $D/binance_api_restrictions.json
bsig GET  /sapi/v1/localentity/questionnaire-requirements '' > $D/binance_questionnaire.json
bsig POST /sapi/v1/asset/get-funding-asset ''              > $D/binance_funding_asset.json
bsig GET  /sapi/v1/asset/wallet/balance ''                 > $D/binance_wallet_balance.json
bsig POST /api/v3/order/test 'symbol=CAKEUSDT&side=BUY&type=LIMIT&timeInForce=IOC&quantity=5&price=1&newClientOrderId=b123456789_test' > $D/binance_order_test.json
bsig POST /api/v3/order/test 'symbol=CAKEUSDT&side=BUY&type=LIMIT&timeInForce=IOC&quantity=5&price=1&newClientOrderId=b123456789_test&computeCommissionRates=true' > $D/binance_order_test_rates.json
bsig POST /api/v3/order/test 'symbol=CAKEUSDT&side=BUY&type=LIMIT&timeInForce=IOC&quantity=5&price=1&newClientOrderId=a%23b' > $D/binance_order_test_bad_id.json  # −1100: допустимый алфавит id
```

Поток аккаунта (`binance_user_stream.jsonl`) — с включённым `venue.balance_events` из сырого журнала `data/raw/binance`,
соединение «account», после депозита/вывода/перевода (кадры `{"subscriptionId", "event": {"e": "balanceUpdate"…}}`).
Первый живой ордер IOC — ответ FULL (`fills`, `commissionAsset`) из сырого журнала (`rest /api/v3/order`).
Затем `cargo test --test binance_real_data -- --ignored` и снять `#[ignore]` с прошедших.

Семейство Solidly V1 — THENA V1 (решение 122, часть F) — tests/bsc_dex_solidly_v1_family.rs: вид solidly_v1 (Proto::SolidlyV1, в графе, без своего пути, шаг SOLIDLY исполнителя, только dex.factory; без exec.executor_address — сигнал), данные вызовов реестра фабрик (getPair(address,address,bool), getPool(…,bool), poolByPair, getDODOPoolBidirection) по официальным сигнатурам. Сверка своей математики Pair.sol с ответом пула getAmountOut — `thena_v1_math_matches_live_get_amount_out`, помечен #[ignore] до записи. **Запись (на сервере, свой узел, ОДИН блок N для всех вызовов, eth_call с тегом N):** по 1 стабильному и 1 волатильному пулу THENA V1 (адрес фабрики — dex.factory DEX THENA V1): `pool.metadata()` (селектор 0x392f37e9), `factory.getFee(bool stable)` (0x512b45ea, stable — из metadata), `pool.getAmountOut(uint256 amountIn, address tokenIn)` (0xf140a35a) — по 3 суммы в обе стороны (малая, ~1% резерва, ~10% резерва). Сохранить в tests/data/thena_v1/responses.json: `{block, pools: [{pool, metadata: "<result>", fee: "<result>", quotes: [{amount_in: "<десятичное>", token_in: "0x…", out: "<result>"}]}]}`; затем `cargo test --test bsc_dex_solidly_v1_family -- --ignored` и снять #[ignore].
Пути проб у пулов без своего Universal Router (решение «122 X (проба через исполнитель)») — tests/bsc_dex_probe_paths.rs: причина пробы налога (шаг исполнителя без exec.executor_address — настройка, вида шага нет — не настройка) на пуле записи CAKE (route_common), откат исполнителя «deadline» — помеха, проба BNB только у видов со своим Universal Router (форк V2, THENA, Topaz — причина без dex.universal_router).

Свой исполнитель ArbExecutor (решение 122, часть X) — tests/bsc_dex_executor_abi.rs: ABI-кодирование execute и шага совпадает с ABI контракта (contracts/out/ArbExecutor.abi: селектор, подпись, поля Step, обратные вызовы), номера видов = константам ArbExecutor.sol (в т.ч. STABLE_U256 = 9, SADDLE = 10), раскладка data видов, селекторы обмена новых видов (exchange(uint256,uint256,uint256,uint256) = 0x5b41b908 — PancakeSwap Stable / Lista, swap(uint8,uint8,uint256,uint256,uint256) — Nerve) по официальным подписям, в IPools.sol и в коде в сети, хеши артефактов и слот владельца; tests/bsc_dex_executor.rs — сборка вызова execute из долей на пулах реальной записи CAKE (route_common): виды шагов, доли в bps от остатка, промежуточный шаг, вход/минимум/срок.

Семейство StableSwap — Ellipsis (решение 122 F, часть 2) — tests/bsc_dex_curve_family.rs: вид curve_stableswap (Proto::CurveStable, вид ядра StableSwap, шаг CURVE), проверка пула по ответам coins(k) в ABI StableSwap.vy. Сверка своей математики (exchange) с ответом пула get_dy — `ellipsis_math_matches_live_get_dy`, #[ignore] до записи. **Запись (свой узел, один блок N, eth_call с тегом N):** пул(ы) Ellipsis из dex.stable_pools (например 3pool BUSD/USDC/USDT): `coins(uint256 k)` (0xc6610657) и `balances(uint256 k)` (0x4903b0d1) для k = 0.. до отката, `A()` (0xf446c1d0), `fee()` (0xddca3f43), `decimals()` (0x313ce567) каждой монеты, `get_dy(int128 i, int128 j, uint256 dx)` (0x5e0d443f) — каждая пара i≠j, по 3 суммы (малая, ~1% и ~10% баланса). Сохранить в tests/data/ellipsis/responses.json: `{block, pools: [{pool, coins: ["<result>"…], balances: ["<result>"…], decimals: [18…], A: "<result>", fee: "<result>", quotes: [{i, j, dx: "<десятичное>", out: "<result>"}]}]}`; затем `cargo test --test bsc_dex_curve_family -- --ignored` и снять #[ignore].

Семейства решения 122 F, 3 — tests/bsc_dex_stable_families.rs: виды pancake_stableswap / lista_stableswap / saddle_stableswap / woofi_v2 (протокол, вид ядра, шаг исполнителя: STABLE_U256 / STABLE_U256 / SADDLE / нет; индексы шага из состояния по токену входа, пул на паузе — не исполняется), свойства общей математики шаблонов (A·100 при A_PRECISION = сырой A; D сбалансированного пула), рампа A, проверка цены Lista. Сверка своей математики с квотером пула — три теста #[ignore] до записи. **Запись (свой узел, один блок N для всех вызовов, eth_call с тегом N), формат файла `{block, pools: [{pool, balances: ["<result>"…], decimals: [18…], amp: "<A в точности шаблона>", fee: "<result>", quotes: [{i, j, dx: "<десятичное>", out: "<result квотера>"}]}]}`; затем `cargo test --test bsc_dex_stable_families -- --ignored` и снять #[ignore]:**

Нативный BNB в стейблсвапах («122 X (нативный BNB в стейблсвапах)») — tests/bsc_dex_stable_native.rs: метка 0xEeee… = BNB_ADDRESS официальных исходников и константа контракта; монеты пула → пара (метка → WBNB), отказ при «метка + WBNB» и без dex.native_wrapped; пара для реестра фабрики (WBNB → метка); decimals метки 18 → множитель 1e18. tests/bsc_dex_executor_abi.rs — номер STABLE_U256_BNB = 11, data (uint256, uint256, bool), receive только от bnbFrom, селекторы WBNB withdraw/deposit в коде, execute не payable; tests/bsc_dex_stable_families.rs — шаг STABLE_U256_BNB из StableData.native (bnb_in по стороне), у Saddle — отказ. Исполнение — только сухой сделкой на узле (docs/OWN_NODE_SWITCH.md, шаг 5): Lista slisBNB/BNB в обе стороны; что смотреть — там же. Записать для теста с узла (tests/data/lista_stable/responses.json, тот же формат): `coins(0)`, `coins(1)`, `balances(k)`, `bnb_gas()` (селектор keccak256("bnb_gas()")), `fetchOraclePrice()` и `get_dy(i, j, dx)` пула 0x3DcEA6AFBA8af84b25F1f8947058AF1ac4c06131 на одном блоке — сверка своей математики и проверки цены с BNB.
- PancakeSwap Stable → tests/data/pancake_stable/responses.json: 1–2 пула фабрики V2 0x25a55f9f2279A54951133D503490342b50E5cd15 (например axlUSDC-USDT 0x6D8fba276ec6F1EDa2344DA48565AdbCA7e4FFa5 — монеты с разными decimals проверяют округление): `coins(uint256)` 0xc6610657, `balances(uint256)` 0x4903b0d1, `A()` 0xf446c1d0 (amp = как есть), `fee()` 0xddca3f43, `decimals()` 0x313ce567 монет, `get_dy(uint256,uint256,uint256)` 0x556d6e9f — обе стороны, по 3 суммы (малая, ~1%, ~10% баланса).
- Lista Stable → tests/data/lista_stable/responses.json: пулы фабрики 0xDE9c8E1536989d8c3817afDabC37C0fb44cB49b4 (USDT/U 0xd77e86779022227226377Dc30D03CF1C78439AcF, solvBTC/BTCB 0x45409865870f0CBC71c01CC00fF8c0b2DE3EB7D9): coins/balances/fee/decimals как выше, `future_A()` 0xb4b577ad (amp = future_A, если `future_A_time()` 0x14052288 ≤ времени блока; иначе записать `initial_A()` 0x5409491a, `initial_A_time()` 0x2081066c и время блока), `get_dy(uint256,uint256,uint256)` 0x556d6e9f по 3 суммы; для проверки цены — `admin_fee()` 0xfee3f7f9, `skipPriceDiff()` 0x2bfdfab4, `price0DiffThreshold()` 0x13e67518, `price1DiffThreshold()` 0x9cf9131f, `fetchOraclePrice()` 0xadd1c817.
- Nerve → tests/data/nerve/responses.json: 3pool 0x1B3771a66ee31180906972580adE9b81AFc5fCDc: `getToken(uint8)` 0x82b86600 до отката, `getTokenBalance(uint8)` 0x91ceb3eb (balances), `getAPrecise()` 0x0ba81959 (amp), `swapStorage()` 0x5fd65f0f (fee — 5-е слово, swapFee), `decimals()` монет, `calculateSwap(uint8,uint8,uint256)` 0xa95b089f по 3 суммы в каждую пару; `paused()` 0x5c975abb.
- WooFi (своей математики нет — проверить, что квотер и обмен есть в развёрнутом коде): `eth_getStorageAt(0x5520385bFcf07Ec87C4c53A7d8d65595Dff69FA4, 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc)` → адрес реализации (ждём 0x7b5fDb8C89A616d319202704B0eA18b168654281); `eth_getCode` реализации — в байткоде есть селекторы `query` 0xf58a435f, `tryQuery` 0xce824f19, `swap(address,address,uint256,uint256,address,address)` 0x7dc20382, `quoteToken()` 0x217a4b70, `wooracle()` 0xbc8530f9, `paused()` 0x5c975abb; у оракула 0x00021b2C857A8613DAEC2fd900122833866828a0 — `state(address)` 0x31e658a5 (ответ 4 слова: price, spread, coeff, woFeasible) по WBNB/BTCB/ETH; `query(base, USDT, 3 суммы)` и `query(USDT, base, …)` на том же блоке — в tests/data/woofi/responses.json (для будущей сверки).
- WooFi, лесенка цен пула (tests/bsc_dex_pmm_ladder.rs, `live_ladder_exact_at_points`, #[ignore] до записи) → tests/data/woofi/ladder.json: на одном блоке N для 1–3 базовых монет (WBNB, BTCB, ETH) — `query(USDT, base, a)` (0xf58a435f на 0x5520385bFcf07Ec87C4c53A7d8d65595Dff69FA4) на суммах лесенки (например 50, 200, 1000, 5000 USDT) → `quote_in: [{in, out}]`; `query(base, USDT, out_k)` на полученных количествах → `base_in`; и по 2–3 суммы МЕЖДУ точками в каждую сторону → `between_quote_in`, `between_base_in`. Формат: `{block, pairs: [{base, quote_in, base_in, between_quote_in, between_base_in}]}` (in/out — десятичные строки или 0x-слова). Затем `cargo test --test bsc_dex_pmm_ladder -- --ignored -- --nocapture`: в точках выход совпадает, между точками печатается отклонение интерполяции — по нему Тигран выбирает шаг лесенки.
Лучший из двух (решение 123) — tests/bsc_dex_best_of.rs: чистый выбор best_of/choose.rs на настоящих котировках CAKE двух блоков (routes_2026-10-29 и routes_2026-10-30 — те же пулы, разная цена): ошибка KyberSwap (RateLimited/Timeout/NoRoute) → своя; ошибки обеих → отказ с причинами; неисполнимый кросс-DEX сплит не берётся; покупка — меньше USDT, продажа — больше; полный объём лучше неполного; худшая цена ядра отсекает сторону; газ без курса BNB — обе без него; отказы «до отправки» (pre_send, одно правило для отката в обе стороны); равенство итогов — своя (tie_goes_to_own). **Записей ответов KyberSwap /routes нет** — в роли KyberSwap котировка другого блока (выбор не знает источника). Тест на настоящем ответе агрегатора — `recorded_kyber_routes_vs_own_same_block`, помечен #[ignore] до записи.

**Запись для него (на сервере, одна монета — CAKE, один момент, свой узел):** каталог `tests/data/kyber_routes/`.
1. Номер блока N: `eth_blockNumber` своего узла — сразу перед запросами к KyberSwap.
2. Пулы CAKE на блоке N — рекордером `record_route.js` теми же вызовами, что routes_2026-10-29 (шесть пулов CAKE/USDT, CAKE/WBNB, WBNB/USDT, CAKE/BTCB, BTCB/WBNB, eth_call с тегом N): `calls.json`, `responses.json` в этот каталог — по ним тест считает свою котировку на том же блоке.
3. `kyber.json`: `{"block": N, "qty": "<монет, целое, например 1000>", "buy": {"amount_in": "<USDT в wei>", "data": <поле data ответа>}, "sell": {"amount_in": "<CAKE в wei = qty·10^18>", "data": <поле data ответа>}}` — ответы `GET {kyber.base_url}/bsc/api/v1/routes?tokenIn=…&tokenOut=…&amountIn=…&gasInclude=true` с заголовком X-Client-Id: продажа — tokenIn CAKE, tokenOut USDT, amountIn = qty CAKE; покупка — tokenIn USDT, tokenOut CAKE, amountIn ≈ USDT продажи (amountOut ответа продажи). Сразу после блока N, в пределах 1–2 блоков. Готовые ответы есть и в сыром журнале бота: `zstdcat ~/arb/data/raw/kyberswap/*.jsonl.zst | grep -m4 routeSummary` — но тогда пулы (п. 2) — на блоке того же времени.
4. `cargo test --test bsc_dex_best_of -- --ignored` и снять #[ignore]. Адреса токенов в записи — реальные; тест сопоставляет с условными адресами route_common только пулы (как routes_2026-10-29).

Семейство Maverick V2 (решение «122 (Maverick V2)») — tests/bsc_dex_maverick_family.rs: вид maverick_v2 (Proto::Maverick, вид ядра TickBin, шаг MAVERICK), проверка пула по ответам isFactoryPool / tokenA / tokenB / permissionedSwap, поведение порта цикла обмена (откат за пределом тика, точный выход больше резерва, MVP-03M — откат активного тика только на ±1, остановка на непрочитанном тике). Сверка порта (math/maverick_swap.rs — порт команды Maverick) с MaverickV2Quoter.calculateSwap — `maverick_port_matches_live_quoter`, #[ignore] до записи. **Запись (свой узел, ОДИН блок N для всех вызовов, eth_call с тегом N; сигнатуры — pools/abi_maverick.rs, селекторы сверяет его тест):** 2–3 пула Maverick V2 на BSC с USDT/WBNB (найти — `lookup(tokenA, tokenB, 0, 20)` у MaverickV2Factory 0x0A7e848Aca42d879EF06507Fca0E7b33A0a63c1e, tokenA < tokenB): у пула `getState()`, `tickSpacing()`, `fee(true)`, `fee(false)`, `tokenAScale()`, `tokenBScale()`; у MaverickV2PoolLens 0x6A9EB38DE5D349Fe751E0aDb4c0D9D391f94cc8D — `getTicksAroundActive(pool, R)` (R с запасом, например 200); у MaverickV2Quoter 0xb40AfdB85a07f37aE217E7D6462e609900dD8D7A — `calculateSwap(pool, amount, tokenAIn, false, tickLimit)`, tickLimit = 2147483647 при tokenAIn = true и −2147483648 при false, по 3 суммы в каждую сторону (малая, ~1% и ~10% резерва входного токена из getState). Для вопросов заметки docs/notes/maverick-v2-research-2026-11-03.md: `eth_getCode` квотера на BSC (сравнить с проверенным на Arbitrum) и `lookup(tokenA, tokenB, 0, 1000)` у пары с 1–2 пулами (откат или обрезка). Сохранить в tests/data/maverick_v2/responses.json: `{block, pools: [{pool, activeTick, reserveA, reserveB, protocolFeeRatioD3, tickSpacing, feeAIn, feeBIn, tokenAScale, tokenBScale, tickRadius, ticks: [{tick, reserveA, reserveB, totalSupply}], quotes: [{amount, tokenAIn, amountIn, amountOut}]}]}` — большие числа десятичными строками, activeTick / tick / tickRadius / protocolFeeRatioD3 — числами, tokenAIn — true/false; затем `cargo test --test bsc_dex_maverick_family -- --ignored` и снять #[ignore].
Исполнение цикла DEX ↔ DEX (решение 124) — tests/bsc_dex_cycle_trade.rs на записи CAKE 2026-10-29 (route_common, блок
130521896), без сети: ноги цикла по сигналу ядра DexCycle (незамкнутый цикл, чужой пул, нативный BNB — отказ), вызов
ArbExecutor для цикла (tokenIn = tokenOut, все доли 10000 bps, minOut — граница ядра), выбор формы (исполнитель →
бандл → отказ с причинами), граница бандла на последнем хопе, курсы по пулам записи, порог чистой прибыли после
симуляции, запрос «базовая → базовая», память причин ядра. **Цикл DEX ↔ DEX: квитанция** — тест
`fact_from_recorded_cycle_receipt` помечен `#[ignore]`: записи живой сделки цикла нет. Что записать с первой живой
сделки цикла (или первой сухой сделки цикла, повторённой живой на малой сумме): `tests/data/cycle_exec_2026-11/receipt.json`
— ответ `eth_getTransactionReceipt` (поле `result`) транзакции execute исполнителя; `meta.json` — `{"wallet": "0x…",
"base": "0x…", "input_units": "…", "output_units": "…"}` (суммы Transfer базовой из кошелька и на кошелёк по BscScan).
Цепочка ядра план → сделка с настоящей площадкой (цена газа eth_gasPrice с узла) тестом не покрыта — нужна запись
ответов узла на одном блоке; проверяется сухой сделкой цикла (docs/OWN_NODE_SWITCH.md, шаг 5а).

Окна (решение 125) — tests/core_windows.rs: логика окна монеты (вход второго направления той же монеты, удержание
window.min_hold_ms, ожидание конца сделки, повторный проход — то же окно), список окон в настоящей SQLite (фильтры и
страницы), настоящее ядро с настоящим адаптером BingX без сети — прошедший сухой кандидат CAKE (size_common/calc.rs)
открывает окно, записанные кадры стакана BingX (tests/data/bingx/ws_spot_2026-10-23.jsonl) идут через приёмник адаптера
в файл окна, окно закрывается, страница окна разбирает кадры адаптером. tests/bingx_window_tap.rs: разбор записанных
стаканов, сделка @trade на примере сообщения из docs-v3 (dataUpdateDemo), строка кадра файла ⇄ кадр.
Нужно записать (тесты #[ignore] до записи): (1) BingX — `tests/data/bingx_trades/trades.jsonl`, строки `{"m": текст}`
сообщений канала `<SYMBOL>@trade` после GZIP (проще всего — открыть окно на сервере бота и взять строки `"s":"Trades"`
из файла окна, поле `t`); (2) MEXC — `tests/data/mexc_ws/deals.hex` (кадры канала
`spot@public.aggre.deals.v3.api.pb@10ms@<SYMBOL>`, по строке hex) и `tests/data/mexc_ws/depth.hex` (кадры
`spot@public.limit.depth.v3.api.pb@<SYMBOL>@20`) — поле `h` строк файла окна.

Окна, дополнение решения 125 — tests/window_depth_diff.rs: кадр BingX @incrDepth на примере из docs-v3 (уровни объектом)
и вид массивом из api-ai-skills, ответ MEXC /api/v3/depth на примере из документации, стакан на момент кадра
(core/window_book.rs) на уровнях записанного кадра BingX depth20. Нужно записать (тесты #[ignore]):
(3) BingX `tests/data/bingx_incr/incr.jsonl` — строки `{"m": текст}` канала `<SYMBOL>@incrDepth` подряд с первого
(action «all»); проверить, объект или массив уровни и есть ли разрывы lastUpdateId; (4) MEXC
`tests/data/mexc_ws/depth_diff.hex` (кадры `spot@public.aggre.depth.v3.api.pb@10ms@<SYMBOL>` подряд, по строке hex — поле
`h` строк потока Diff файла окна) и `tests/data/mexc_ws/depth_snapshot.json` (строка потока Snapshot того же окна, поле
`t`). На первой записи сделок MEXC (deals.hex) проверить, что tradeId у всех сделок разные (решение 125, дополнение) —
добавить проверку в recorded_deals_decode.

PancakeSwap Infinity Stable (решение «122 (Infinity Stable)») — tests/bsc_dex_inf_stable.rs: вид pancake_infinity_stable (Proto::InfinityStable, StableSwap, в графе, tradable через Universal Router, без шага исполнителя), селекторы крючка, свойства math/stableswap_ng.rs (динамическая комиссия, D сбалансированного пула, NG ≈ шаблон A_PRECISION при offpeg ≤ 1e10). Сверка своей математики _exchange_received с get_dy крючка — `ng_math_matches_live_get_dy`, #[ignore] до записи. **Запись (свой узел, ОДИН блок N для всех вызовов, eth_call с тегом N):** крючок(и) из dex.stable_hooks (минимум 0x86c3DC08FB5a6663cCa15551575d5429e5Efc017): `balances(uint256)` 0x4903b0d1 для 0 и 1, `stored_rates()` 0xfd0684b1 (сырой result целиком), `A_precise()` 0x76a2f0f0, `fee()` 0xddca3f43, `offpeg_fee_multiplier()` 0x8edfdd5f, `get_dy(int128,int128,uint256)` 0x5e0d443f — i=0→j=1 и i=1→j=0, по 3 суммы (малая, ~1% и ~10% баланса монеты i). Желательно пул, где offpeg > 1e10 и балансы не равны (проверяет динамическую комиссию), и пул с монетой с курсом (rates ≠ 10^(36−decimals)). Сохранить в tests/data/infinity_stable/responses.json: `{block, pools: [{hook, balances: ["<result>", "<result>"], stored_rates: "<result>", A_precise: "<result>", fee: "<result>", offpeg_fee_multiplier: "<result>", quotes: [{i, j, dx: "<десятичное>", out: "<result get_dy>"}]}]}`; затем `cargo test --test bsc_dex_inf_stable -- --ignored` и снять #[ignore].

Ответы Тиграна по Infinity Stable (решение «122 (Infinity Stable, ответы)») — tests/bsc_dex_inf_stable_answers.rs: крючок из dex.stable_hooks видом Infinity CL не берётся (Initialize → нет пула CL, есть пул Infinity Stable; чужой крючок — наоборот); основной квотер — CLQuoter (dex.quoter, ключ с крючком), дополнительный — get_dy крючка с индексами по токену входа; у других видов дополнительного нет. Записей не требует.

Решение 129 — tests/core_calc_timing.rs (замер этапов расчёта кандидата: части одного имени складываются, запросы к узлу — в свою часть, отказ по сроку называет этап и часть и даёт ключ повтора без чисел; данных площадок нет — только учёт времени), tests/bsc_dex_graph.rs cached_graph_rebuilds_only_on_change (граф маршрутов из кэша, пока не менялись реестр и блок; после записи и нового блока — новый, равный построенному заново).

Решение 131: tests/bsc_dex_quote_cpu.rs — котировка DEX на большом реестре (индексы пары и тикера, общие карты тиков, тот же итог, время) и счёт вне рабочих потоков (другая задача рантайма получает управление, срок срабатывает вовремя) — запись маршрута CAKE (routes_2026-10-29), лишние пулы — то же записанное состояние под условными адресами монет; tests/core_cold_coins.rs — справочник монет частями (рабочие / неподвижные) равен целому и монета с новой сетью BSC переходит в рабочие без перезапуска — живые ответы BingX 2026-10-24 (config/getall, symbols) и стакан CAKE 2026-10-23.

Решение 132 (маппинг кошельков): tests/wallet_accounts.rs — хранилище кошельков и маппинга (перенос один раз, запрет удаления, история); tests/bsc_dex_accounts.rs — адаптер DEX на счёт (свой ключ, адрес депозита, исполнитель и очередь у каждого кошелька; без счёта — отказ); tests/core_wallet_mapping.rs — ядро (кошелёк направления по маппингу, MEXC без строки не торгует с DEX, сделка помнит кошелёк, кошелёк переноса для старых сделок); tests/api_wallets.rs — API (ключ не выходит в ответы, историю и журнал; тот же адрес — отказ). Ключи — официальные тестовые векторы (EIP-155 «Example», Hardhat Network счёт #0), исполнитель — записанный адрес 0x9ef558a5d437718dde44e00990e0efb9976cf794.
Обе стороны до покупки (решение 134) — tests/sell_precheck_rules.rs: правила ордера продажи MEXC и BingX на записанных exchangeInfo 2026-11-02 и common/symbols 2026-10-24 (успех, отказ по типу, стороне API, минимуму, максимуму, шагам); настоящий ордер и проверка без исполнения зовут одну функцию order_rules::ready.
Биржа Gate (решение 135) — tests/gate_real_data.rs на живых публичных ответах 2026-11-04 (tests/data/cex_samples_2026_11_04/gate_*): весь справочник пар (шаги 10^−precision, минимумы, белый список ключа, buy_start), весь справочник монет (сеть «BSC», адрес контракта в смешанном регистре, закрытые сети), currency_chains CAKE (decimal 8 сильнее точности пары), снимок REST для окна, кадры WebSocket (ответы на подписку, spot.pong, spot.order_book — 20 уровней, lastUpdateId не убывает; spot.order_book_update и spot.trades для окна), правила ордера ioc на живой паре и свой text «t-…». Приватные — #[ignore], что записать на живом боте: GET /wallet/withdraw_status (весь и ?currency=CAKE: withdraw_fix_on_chains["BSC"], формат withdraw_percent_on_chains); /account/detail (key.mode, ip_whitelist, currency_pairs) и /account/main_keys (perms: имена, read_only, есть ли право вывода); /spot/accounts (весь и ?currency=USDT); ордер ioc на минимальную сумму CAKE_USDT покупкой и продажей (ответ POST /spot/orders, GET /spot/orders/{id}, GET /spot/orders?status=finished — text, fee, gt_fee, point_fee, finish_as); /wallet/deposit_address?currency=CAKE; вывод POST /withdrawals и GET /wallet/withdrawals?withdraw_id= по шагам статуса; /wallet/deposits (TRACK → DONE / DEP_CREDITED — оба «зачислено»); вывод в статусе FAIL (событие exec.withdraw_fail_pending); кадры spot.balances при депозите, ордере и выводе; GET /wallet/fee?currency_pair=CAKE_USDT при скидке GT включённой и выключенной (`wallet_fee_gt`: taker_fee, gt_discount, gt_taker_fee → fees::Rates::parse и plan для coin / gt) и fee_currency ордера при каждом варианте.
Биржа Gate (решение 135) — tests/gate_real_data.rs на живых публичных ответах 2026-11-04 (tests/data/cex_samples_2026_11_04/gate_*): весь справочник пар (шаги 10^−precision, минимумы, белый список ключа, buy_start), весь справочник монет (сеть «BSC», адрес контракта в смешанном регистре, закрытые сети), currency_chains CAKE (decimal 8 сильнее точности пары), снимок REST для окна, кадры WebSocket (ответы на подписку, spot.pong, spot.order_book — 20 уровней, lastUpdateId не убывает; spot.order_book_update и spot.trades для окна), правила ордера ioc на живой паре и свой text «t-…». Приватные — #[ignore], что записать на живом боте: GET /wallet/withdraw_status (весь и ?currency=CAKE: withdraw_fix_on_chains["BSC"], формат withdraw_percent_on_chains); /account/detail (key.mode, ip_whitelist, currency_pairs) и /account/main_keys (perms: имена, read_only, есть ли право вывода); /spot/accounts (весь и ?currency=USDT); ордер ioc на минимальную сумму CAKE_USDT покупкой и продажей (ответ POST /spot/orders, GET /spot/orders/{id}, GET /spot/orders?status=finished — text, fee, gt_fee, point_fee, finish_as); /wallet/deposit_address?currency=CAKE; вывод POST /withdrawals и GET /wallet/withdrawals?withdraw_id= по шагам статуса; /wallet/deposits (TRACK → DONE / DEP_CREDITED); кадры spot.balances при депозите, ордере и выводе.

Решение 141 — tests/core_uncapped.rs (расчёт «без потолка» на записанных стакане и пуле CAKE: не хуже, чем с потолком и чем боевой подбор на том же бюджете; минимум до плюса совпадает с прямым расчётом кривой и на шаг меньше не проходит; «не становится плюсовой»; упущено = 0 при не ограничивающем бюджете, > 0 при ограничивающем; таблица потолков не убывает), tests/core_uncapped_peaks.rs (многопиковая кривая из записи CAKE — две сделки подряд на тех же записях: два пика, оптимум на втором, не хуже перебора; замер стоимости при разных uncapped.max_evals).

## OKX (решение 161) — tests/okx_real_data.rs

Без сети: подпись по формуле официального SDK github.com/okxapi/python-okx v0.4.4 (`okx/utils.py` signature,
`okx/websocket/WsUtils.py` initLoginParams) на примере документации «REST Authentication»
(`GET /api/v5/account/balance?ccy=BTC`, время `2020-12-08T09:08:57.715Z`), POST ордера и вход WebSocket. Живых ответов
OKX нет (из среды разработки www.okx.com закрыт прокси) — тесты на записях `#[ignore]`, пока не записаны файлы
`tests/data/cex_samples_2026_11_05/okx_*`. Записывать на сервере бота; `R` — домен REST сущности счёта
(значение `venue.rest_url`: основной `https://openapi.okx.com`, EEA `https://eea.okx.com`, US `https://us.okx.com`),
`W` — WebSocket public той же сущности (`venue.ws_url`, основной `wss://ws.okx.com/ws/v5/public`).

Публичные (без ключа):

```bash
D=tests/data/cex_samples_2026_11_05; mkdir -p $D
curl -sS --compressed "$R/api/v5/public/instruments?instType=SPOT" -o $D/okx_instruments_spot.json
curl -sS --compressed "$R/api/v5/market/books?instId=CAKE-USDT&sz=400" -o $D/okx_books_rest_CAKE-USDT.json
(echo '{"op":"subscribe","args":[{"channel":"books5","instId":"CAKE-USDT"}]}'; sleep 60) \
  | websocat -t "$W" > $D/okx_books5.jsonl
(echo '{"op":"subscribe","args":[{"channel":"books","instId":"CAKE-USDT"}]}'; sleep 120) \
  | websocat -t "$W" > $D/okx_books.jsonl
```

Приватные (ключ бота; переменные `OKX_KEY`, `OKX_SECRET`, `OKX_PASS` — только в оболочке, в файлы не писать). Подпись —
как sign.rs: Base64(HMAC-SHA256(время + GET + путь?запрос)):

```bash
okx() { TS=$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)
  SIGN=$(printf '%s' "${TS}GET$1" | openssl dgst -sha256 -hmac "$OKX_SECRET" -binary | base64)
  curl -sS --compressed "$R$1" -H "OK-ACCESS-KEY: $OKX_KEY" -H "OK-ACCESS-SIGN: $SIGN" \
    -H "OK-ACCESS-TIMESTAMP: $TS" -H "OK-ACCESS-PASSPHRASE: $OKX_PASS"; }
okx '/api/v5/asset/currencies?ccy=CAKE,USDT' > $D/okx_currencies_CAKE.json
okx '/api/v5/asset/currencies' > $D/okx_currencies_all.json            # размер ответа (урок 9)
okx '/api/v5/asset/deposit-address?ccy=CAKE' > $D/okx_deposit_address_CAKE.json
okx '/api/v5/asset/deposit-history?ccy=CAKE' > $D/okx_deposit_history.json
okx '/api/v5/asset/withdrawal-history?ccy=CAKE' > $D/okx_withdrawal_history.json
okx '/api/v5/account/trade-fee?instType=SPOT&instId=CAKE-USDT' > $D/okx_trade_fee_CAKE.json
okx '/api/v5/account/config' > $D/okx_account_config.json             # перед коммитом заменить uid, mainUid, label, ip
okx '/api/v5/account/balance?ccy=USDT,CAKE' > $D/okx_account_balance.json
okx '/api/v5/asset/balances' > $D/okx_funding_balances.json
```

С живого бота (сырой журнал `data/raw/okx`, без ключей): соединение «account» — кадры канала deposit-info
(`okx_deposit_info.jsonl`: депозит state 0 → 1 → 2), «rest /api/v5/trade/order» — первый ордер ioc CAKE-USDT покупкой и
продажей (ответ постановки и GET ордера: sCode, accFillSz, avgPx, fee, feeCcy), «rest /api/v5/asset/transfer» и
transfer-state, «rest /api/v5/asset/withdrawal» и withdrawal-history по шагам state. Затем
`cargo test --test okx_real_data -- --ignored` и снять `#[ignore]` с записанных.

Что проверить на записях (документация прочитана пересказчиком): имя сети BSC в chain (`CAKE-BSC`?) и полный ли
`ctAddr` в currencies; `canDep` / `canWd` — boolean или строка; пустой ли `minDep`; `to` адреса депозита — «6» / «18»;
знак `taker` в trade-fee (отрицательный — комиссия); `ruleType` у спот-пар; порядок уровней и пустые кадры books
(seqId = prevSeqId); есть ли в perm права перевода; 51020 — минимальная сумма ордера CAKE-USDT.

## XT (решение 167) — tests/xt_real_data.rs

Всегда выполняются: сборка строки подписи на примере документации (signSteps.md, «sample of original signature
message»; X фьючерсов), HMAC SHA256 на векторе RFC 4231 (подписи из примеров XT не воспроизводятся — секрет второго
примера не опубликован, тело первого не JSON), человеческая причина кода `AUTH_103` (решение 164). Остальное —
`#[ignore]` до записи в `data/cex_samples_2026_11_05/xt_*` (из среды Claude `sapi.xt.com` закрыт прокси).

### XT — команды записи (на сервере бота)

Публичные (без ключа):

```bash
D=tests/data/cex_samples_2026_11_05; mkdir -p $D
curl -sS 'https://sapi.xt.com/v4/public/symbol?symbol=cake_usdt' -o $D/xt_symbol_cake_usdt.json
curl -sS 'https://sapi.xt.com/v4/public/symbol' -o $D/xt_symbol_all.json      # размер, version
V=$(python3 -c "import json;print(json.load(open('$D/xt_symbol_all.json'))['result']['version'])")
curl -sS "https://sapi.xt.com/v4/public/symbol?version=$V" -o $D/xt_symbol_same_version.json  # что приходит без изменений
curl -sS 'https://sapi.xt.com/v4/public/wallet/support/currency' -o $D/xt_support_currency.json
curl -sS 'https://sapi.xt.com/v4/public/currencies' -o $D/xt_currencies.json
curl -sS 'https://sapi.xt.com/v4/public/depth?symbol=cake_usdt&limit=500' -o $D/xt_depth_cake_usdt.json
# WebSocket: python websockets сам договаривается о permessage-deflate (pip install websockets)
python3 - <<'PY' > $D/xt_ws_depth_update.jsonl
import asyncio, json, websockets
async def main():
    async with websockets.connect("wss://stream.xt.com/public") as ws:
        await ws.send(json.dumps({"method":"subscribe","params":["depth_update@cake_usdt"],"id":"1"}))
        end = asyncio.get_event_loop().time() + 20
        while asyncio.get_event_loop().time() < end:
            print(await ws.recv(), flush=True)
asyncio.run(main())
PY
# сразу после — снимок REST ещё раз (для local_book_from_live_frames нужен снимок в середине потока):
curl -sS 'https://sapi.xt.com/v4/public/depth?symbol=cake_usdt&limit=500' -o $D/xt_depth_cake_usdt.json
# снимки depth@cake_usdt,20 — тем же скриптом с params ["depth@cake_usdt,20"] → $D/xt_ws_depth.jsonl
# без сжатия (так подключается бот: tokio-tungstenite не умеет deflate) — принимает ли сервер:
{ echo '{"method":"subscribe","params":["depth@cake_usdt,20"],"id":"1"}'; sleep 10; } \
  | websocat -t 'wss://stream.xt.com/public' > $D/xt_ws_depth_no_deflate.jsonl
```

Приватные (ключ XT; ключ и подпись в файлы не попадают — только тела ответов):

```bash
K='<AppKey>'; S='<SecretKey>'; B=https://sapi.xt.com
xsig() { # $1 метод, $2 путь, $3 строка запроса (ключи по алфавиту, может быть пусто), $4 тело JSON (может быть пусто)
  local ts=$(date +%s%3N) y="#$1#$2${3:+#$3}${4:+#$4}"
  local x="validate-algorithms=HmacSHA256&validate-appkey=$K&validate-recvwindow=5000&validate-timestamp=$ts"
  local s=$(printf '%s' "$x$y" | openssl dgst -sha256 -hmac "$S" | sed 's/^.* //')
  curl -sS -X "$1" -H 'Content-Type: application/json' -H "validate-algorithms: HmacSHA256" -H "validate-appkey: $K" \
    -H "validate-recvwindow: 5000" -H "validate-timestamp: $ts" -H "validate-signature: $s" \
    ${4:+--data-raw "$4"} "$B$2${3:+?$3}"; }
xsig GET /v4/balances ''                                   > $D/xt_balances.json   # AUTH_103 — попробовать xt-validate-* (SDK)
xsig GET /v4/balance 'currency=usdt'                       > $D/xt_balance_usdt.json
xsig GET /v4/deposit/address "chain=<сеть BSC>&currency=cake" > $D/xt_deposit_address_cake.json
xsig GET /v4/deposit/history "chain=<сеть BSC>&currency=usdt&limit=200" > $D/xt_deposit_history.json
xsig GET /v4/withdraw/history "chain=<сеть BSC>&currency=usdt&limit=200" > $D/xt_withdraw_history.json
# первый IOC-ордер (маленький, цена ниже рынка — не исполнится): ответ, затем состояние
xsig POST /v4/order '' '{"symbol":"cake_usdt","side":"BUY","type":"LIMIT","timeInForce":"IOC","bizType":"SPOT","price":0.5,"quantity":10,"clientOrderId":"b123456789_test"}' > $D/xt_order_post.json
xsig GET /v4/order 'clientOrderId=b123456789_test'         > $D/xt_order.json     # state IOC: CANCELED или EXPIRED
B=https://api.xt.com xsig GET /v4/user/account/api-key ''  | python3 -c "import json,sys;d=json.load(sys.stdin);[r.pop('secretKey',None) for r in (d.get('result') or [])];print(json.dumps(d))" > $D/xt_api_key.json
```

Фьючерсы (своя подпись: X = `validate-appkey=…&validate-timestamp=…`, Y = `#path`) — `xt_futures_balance.json` с
`https://fapi.xt.com/future/user/v1/balance/list`. Исполненный ордер с комиссией (deductServices) — `xt_order.json` после
первой живой сделки, из сырого журнала data/raw/xt («rest /v4/order/<id>»).
