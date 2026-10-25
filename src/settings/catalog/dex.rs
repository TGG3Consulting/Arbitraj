//! Файл: settings/catalog/dex.rs
//! Что делает: настройки адаптера DEX — узел BSC, режим обмена, пулы, проверка расчёта, разрешения,
//! умный режим и адреса контрактов каждого вида пула. Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, решения 29, 31, 38).
//!
//! Адреса контрактов — только из официальной документации DEX (research/flow/bsc_dex.md, bsc_v4.md,
//! bsc_smart_router.md); вводит Тигран. Область «DEX» (dx): id — id DEX у индексатора (решение 58, таблица
//! сопоставления dex.kind), например «pancakeswap-v3-bsc».

use super::{DX, G, V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Варианты dex.kind — единственный список: по нему же адаптер DEX выбирает модуль пулов (pools/families.rs, тест
/// сверяет оба направления). Это перечень модулей адаптера, а не список DEX (решения 58, 59).
pub const DEX_KINDS: &[&str] = &[
    "pancakeswap_v2",
    "pancakeswap_v3",
    "pancakeswap_infinity_cl",
    "pancakeswap_infinity_bin",
    "uniswap_v2",
    "uniswap_v3",
    "uniswap_v4",
    "v2_fork",
    "v3_fork",
    "v3_fork_pancake",
    "unsupported",
];

/// Варианты dex.universal_router_version — форматы команд Universal Router (exec/ur.rs).
pub const UR_FORMATS: &[&str] = &["pancakeswap_infinity", "2.0", "2.1.1", "2.1.2"];

/// Настройки DEX. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static DEX: &[SettingDef] = &[
    d("Узел BSC", "bsc.pool_events_source", K::Choice, L::None, "источник", G, "Решение 68: откуда события пулов (и новых пулов): ws — подписка logs по WebSocket узла (bsc.node_ws_url); receipts — квитанции каждого блока eth_getBlockReceipts по HTTP узла (bsc.node_http_url, очередь bsc.node_http_rps, класс проверок), логи фильтруются у нас. receipts — 2 вызова на блок (заголовок и квитанции; BSC ≈ 2,2 блока/с). Не задана — пулы не слушаются").choices(&["ws", "receipts"]),
    d("Узел BSC", "bsc.receipts_max_lag_blocks", K::Integer, L::Positive, "блоков", G, "Решения 68, 72, источник receipts: лента отстала от последнего блока больше чем на столько — котировки и сделки запрещены («данные пулов отстают на N блоков»; отметка снимается при отставании ≤ этого значения), лента догоняет без сброса, состояние пулов сохраняется, проверки (квотер, крючки, налоги) вердикт не выносят; полный сброс — только если пропуск невосполним (реорг глубже окна, блок не отдаёт ни один узел)"),
    d("Узел BSC", "bsc.receipts_stuck_reset_s", K::Integer, L::Positive, "секунды", G, "Решение 72, источник receipts: лента стоит на одном блоке дольше стольких секунд (например, тяжёлый блок больше bsc.receipts_max_body_bytes или дольше bsc.receipts_total_ms на всех узлах) — запись, событие dex.receipts_stuck в Telegram с причиной (предел и размер) и полный сброс. Не задана — застрявший блок не сбрасывается (одна запись в журнал)"),
    d("Узел BSC", "bsc.receipts_node_urls", K::TextList, L::None, "адреса https", G, "Решение 72, источник receipts: публичные узлы HTTP для ленты (из официального списка BNB Chain, docs.bnbchain.org json-rpc-endpoint) — блоки конвейера по кругу на разные узлы, повтор блока, на котором стоит лента, — гонкой на все. Пусто или не задана — лента через основной узел bsc.node_http_url и очередь bsc.node_http_rps"),
    d("Узел BSC", "bsc.receipts_node_rps", K::Integer, L::Positive, "вызовов в секунду", G, "Решение 72, источник receipts: темп КАЖДОГО узла списка bsc.receipts_node_urls (у публичного узла свой лимит; пачка блока — 3 вызова: заголовок, квитанции, голова узла). Обязательна, если список задан"),
    d("Узел BSC", "bsc.receipts_reorg_depth", K::Integer, L::Positive, "блоков", G, "Решение 68, источник receipts: сколько последних блоков помнить для реорга (сверка parentHash); реорг глубже — пулы недостоверны, лента заново"),
    d("Узел BSC", "bsc.receipts_retry_ms", K::Integer, L::Positive, "мс", G, "Решение 68, источник receipts: сбой узла (нет блока, нет связи, 429), неполные квитанции, ответ больше предела — тот же блок снова через столько мс; блоки не пропускаются. Не задана — лента не идёт (пулы недостоверны, причина в журнале)"),
    d("Узел BSC", "bsc.receipts_ahead_blocks", K::Integer, L::Positive, "блоков", G, "Решение 69, источник receipts: окно конвейера — сколько следующих блоков запрашивать параллельно (заголовок и квитанции блока — одним JSON-RPC batch, 2 вызова); раздаются строго по порядку, не пришёл блок — он же снова, следующие ждут. Последовательно лента не успевает за сетью (≈0,35 с на запрос против блока раз в ≈0,45 с). Запросы идут через очередь bsc.node_http_rps. Не задана — лента не идёт"),
    d("Узел BSC", "bsc.receipts_max_body_bytes", K::Integer, L::Positive, "байт", G, "Решение 68, источник receipts: предел размера ответа eth_getBlockReceipts (только для квитанций; блок BSC — 0,4–0,7 МБ, замер 25.10). Больше — запись «блок N: квитанции больше предела» и повтор; повторяется — отставание, пулы недостоверны"),
    d("Узел BSC", "bsc.receipts_total_ms", K::Integer, L::Positive, "мс", G, "Решение 68, источник receipts: общий срок ответа eth_getBlockReceipts (только для квитанций; вместо http.total_ms). Дольше — запись «блок N: квитанции больше предела» и повтор"),
    d("Узел BSC", "bsc.node_ws_url", K::Url, L::None, "URL", G, "WebSocket узла BSC (подписки logs; номер блока — опросом по HTTP, решение 57); ключ провайдера в адресе — как {key}, сам ключ — секрет bsc.node_ws_key"),
    d("Узел BSC", "bsc.node_background_share_pct", K::Decimal, L::Percent, "%", G, "Решение 61: гарантированная доля мест очереди узла HTTP для фона (загрузка и проверка пулов, цикл квотера), пока он ждёт; остальное — строгий порядок: сделка → проверки монет → котировки → фон. 0 или не задана — фон получает место, только когда старших в очереди нет"),
    d("Узел BSC", "bsc.node_http_rps", K::Integer, L::Positive, "вызовов в секунду", G, "Темп узла HTTP: сколько вызовов JSON-RPC в секунду (именно вызовов, не CU провайдера — значение подбирается под тариф); пачка из N вызовов — N; на «превышен лимит» (429) — правило шлюза «Превышен лимит запросов»"),
    d("Узел BSC", "bsc.blocks_poll_ms", K::Integer, L::Positive, "мс", G, "Опрос номера блока eth_blockNumber по HTTP (решение 57: вместо подписки newHeads): раз в столько мс; меньше feed.stale_after_ms. Идёт мимо очереди bsc.node_http_rps — к узлу добавляется 1000/значение вызовов в секунду (5 CU каждый у NodeReal)"),
    d("Узел BSC", "bsc.node_http_url_backup", K::Url, L::None, "URL", G, "Резервный HTTP узла BSC (другой провайдер): сбой основного (нет связи, таймаут, 5xx, 429) — тот же запрос сюда; ключ — как {key}, секрет bsc.node_http_key_backup. Не задан — только основной"),
    d("Узел BSC", "bsc.node_ws_url_backup", K::Url, L::None, "URL", G, "Резервный WebSocket узла BSC: основной не подключился — подключение сюда; ключ — как {key}, секрет bsc.node_ws_key_backup. Не задан — только основной"),
    d("Узел BSC", "bsc.node_backup_hold_s", K::Integer, L::Positive, "секунды", G, "После сбоя основного узла запросы (HTTP и подключения WS — отдельно) идут на резервный столько секунд, потом снова сначала основной. Не задана — на резервный уходит только сам неудачный запрос"),
    d("Узел BSC", "bsc.node_http_url", K::Url, L::None, "URL", G, "HTTP узла BSC (eth_call, eth_getLogs, eth_estimateGas, квитанции); ключ — как {key}, секрет bsc.node_http_key"),
    d("DEX", "dex.multicall3", K::Address, L::None, "адрес", V, "Multicall3 (github.com/mds1/multicall3): пачка чтений — один eth_call вместо N (у NodeReal 20 CU за eth_call). Не задан — пачкой JSON-RPC (N вызовов), поиск пулов — по вызову на пару"),
    d("DEX", "dex.swap_mode", K::Choice, L::None, "режим", V, "Режим обмена: прямой (лучший пул) / умный (маршрут через Universal Router) (решение 29)").choices(&["direct", "smart"]),
    d("DEX", "dex.kind", K::Choice, L::None, "вид", DX, "Таблица сопоставления (решения 58, 59): вид пулов DEX индексатора (id области — id DEX у индексатора, например pancakeswap-v3-bsc). В сделки идут только виды с исполнением через Universal Router своей фабрики: pancakeswap_v2, pancakeswap_v3, pancakeswap_infinity_cl, pancakeswap_infinity_bin, uniswap_v2, uniswap_v3, uniswap_v4. Форки (v2_fork, v3_fork, v3_fork_pancake) — пока «не подключена к сделкам: нет своего пути исполнения»; unsupported — не поддерживается. Не задан — пулы DEX не берутся").choices(DEX_KINDS),
    d("DEX", "dex.pool_enabled", K::Bool, L::None, "да/нет", DX, "DEX включена (id области — id DEX у индексатора). Нет — её пулы не берутся из справочника и не торгуются"),
    d("DEX", "dex.intermediate_enabled", K::Bool, L::None, "да/нет", V, "Промежуточные монеты (решение 52): да — маршрут «USDT → промежуточная → монета» в одной транзакции и монеты с пулом только с промежуточной допускаются; нет — только пулы с USDT"),
    d("DEX", "dex.inflow_wait_notify_s", K::Integer, L::Positive, "секунды", V, "Обмен ждёт ожидаемого поступления того же токена в кошелёк дольше — одно уведомление в Telegram (dex.swap_waits_inflow)"),
    d("DEX", "dex.native_ticker", K::Text, L::None, "тикер", V, "Как биржа называет нативную монету BSC (BNB): этим тикером записываются затраты шага через её обёртку (WETH() роутера V2 своей DEX, WBNB 1:1)"),
    d("DEX", "dex.native_wrapped", K::Address, L::None, "адрес", V, "Решение 62: адрес обёртки нативной монеты BSC (WBNB) — монета биржи с тикером dex.native_ticker в сети BSC без адреса контракта (BNB) сопоставляется с этим токеном: пулы, котировка и проба — по нему; продажа — WRAP_ETH в той же транзакции Universal Router, покупка — UNWRAP_WETH на кошелёк, перевод на биржу — нативный. Не задан — нативная монета не торгуется (как раньше: нет адреса контракта)"),
    d("DEX", "dex.intermediate_tokens", K::AddressList, L::None, "адреса BSC", V, "Промежуточные монеты (решение 52): адреса токенов в BSC (например WBNB). Нужен, если dex.intermediate_enabled = да"),
    d("DEX", "dex.min_pool_liquidity_usd", K::Decimal, L::NonNegative, "USDT", V, "Решение 73: порог глубины пула ПО НАПРАВЛЕНИЮ в полосе цены dex.depth_band_pct — покупка (USDT → монета) и продажа (монета → USDT) считаются отдельно; котировка и сделка берут пул, глубокий по своей стороне, сверка с квотером — по покупке, проба налогов — по обеим. Мелкие пулы (по обеим сторонам) при источнике ws слушаются только по добавлению ликвидности; при receipts пулы не спят"),
    d("DEX", "dex.depth_band_pct", K::Decimal, L::Percent, "%", V, "Решение 73: полоса цены для глубины пула — сколько USDT пул примет (покупка) или отдаст (продажа), пока цена монеты не сдвинется на столько процентов; больше 0 и меньше 100. Слова битмапа тиков при загрузке — с запасом на полосу (не меньше dex.tick_words). Не задана — глубина не считается, пулы не котируются"),
    d("DEX", "dex.depth_hysteresis_pct", K::Decimal, L::Percent, "%", V, "Решение 73: гистерезис глубины — мелкое направление становится глубоким при глубине ≥ dex.min_pool_liquidity_usd, а глубокое мелким — только ниже порога × (1 − этот % / 100); пул не мигает на границе. 0 — без гистерезиса. Не задана — глубина не считается"),
    d("DEX", "dex.depth_log_min_s", K::Integer, L::NonNegative, "с", V, "Решение 73: запись в журнал о смене «глубокий ↔ мелкий» по направлению — только при реальной смене и по одному пулу не чаще, чем раз в столько секунд. Не задана — глубина не считается"),
    d("DEX", "dex.pool_load_retries", K::Integer, L::Positive, "попыток", V, "Состояние пула не прочиталось при подключении (сбой узла): сколько раз повторить чтение — по разу на каждый новый блок; дальше пул недостоверен до переподключения подписки"),
    d("DEX", "dex.verify_batch", K::Integer, L::Positive, "вызовов", V, "Проверка пулов справочника на узле (решение 58): сколько вызовов в одном eth_call Multicall3 (или в одной пачке JSON-RPC без него); сбой одной части не мешает остальным"),
    d("DEX", "dex.load_batch", K::Integer, L::Positive, "вызовов", V, "Решение 61: чтение состояния пулов пачками — сколько вызовов в одном eth_call Multicall3 (без dex.multicall3 — в одной пачке JSON-RPC); пулы читаются группами по половине этого числа, у каждой группы свежий блок. Не задана — состояние пулов не читается (причина в журнале)"),
    d("DEX", "dex.verify_retry_s", K::Integer, L::Positive, "секунды", V, "Проверка пулов справочника не выполнена из-за узла (нет связи, 429): повтор через столько секунд"),
    d("DEX", "dex.pools_per_sub", K::Integer, L::Positive, "штук", V, "Сколько пулов в одной подписке logs (узел BSC: до 1000 вариантов в позиции topics)"),
    d("DEX", "dex.logs_block_batch", K::Integer, L::Positive, "блоков", V, "Сколько блоков в одном eth_getLogs при чтении истории Initialize пулов V4 (не больше лимита провайдера узла)"),
    d("DEX", "dex.history_scan", K::Bool, L::None, "да/нет", V, "Необязательно (решение 58: пулы V4 и Infinity — из справочника, проверка по ключу у менеджера). Читать историю пулов V4 (разовое чтение Initialize, eth_getLogs) — нужна только для пулов Uniswap V4 без позиций через PositionManager: нет или не задана — не читается (место сохраняется, найденное слушается)"),
    d("DEX", "dex.scan_progress_s", K::Integer, L::Positive, "секунды", V, "Чтение истории пулов V4: ход (прочитано, осталось блоков) — в журнал не чаще раза в столько секунд; найденные пулы подключаются к подписке с той же частотой"),
    d("DEX", "dex.tick_words", K::Integer, L::Positive, "слов", V, "V3/CL: сколько слов битмапа тиков читать вокруг текущего тика"),
    d("DEX", "dex.bin_depth_usd", K::Decimal, L::Positive, "USDT", V, "Bin: на какую глубину в USDT читать бины в обе стороны от активного"),
    d("DEX", "dex.quoter_max_diff_pct", K::Decimal, L::Percent, "%", V, "Расхождение расчёта с квотером / eth_call больше — стоп по пулу, сделки нет"),
    d("DEX", "dex.quoter_check_s", K::Integer, L::Positive, "секунды", V, "Как часто сверять локальный расчёт пулов с квотером"),
    d("DEX", "dex.deadline_s", K::Integer, L::Positive, "секунды", V, "Срок жизни транзакции обмена (deadline роутера)"),
    d("DEX", "dex.approve_unlimited", K::Bool, L::None, "да/нет", V, "Разрешение (approve) на максимальную сумму один раз; нет — на сумму каждой сделки"),
    d("DEX", "dex.permit2_expiration_s", K::Integer, L::Positive, "секунды", V, "Срок разрешения Permit2 → Universal Router"),
    d("Умный режим", "smart.universal_router", K::Address, L::None, "адрес", V, "Universal Router PancakeSwap Infinity, исполнитель умного маршрута (решение 38)"),
    d("Умный режим", "smart.permit2", K::Address, L::None, "адрес", V, "Permit2 PancakeSwap для умного режима"),
    d("Умный режим", "smart.mixed_quoter", K::Address, L::None, "адрес", V, "MixedQuoter PancakeSwap — сверка умного маршрута"),
    d("Умный режим", "smart.max_splits", K::Integer, L::Positive, "долей", V, "На сколько долей можно делить вход"),
    d("Умный режим", "smart.split_step_pct", K::Decimal, L::Percent, "%", V, "Шаг доли при поиске деления входа"),
    d("Контракты DEX", "dex.factory", K::Address, L::None, "адрес", DX, "V2/V3: фабрика пулов этой DEX — проверка каждого пула из справочника (factory() пула и getPair/getPool фабрики) и события создания новых пулов"),
    d("Контракты DEX", "dex.router", K::Address, L::None, "адрес", DX, "V2: Router — сверка расчёта пула (getAmountsOut) и обёртка нативной монеты (WETH()); виды V3 и V4 его не используют (обмен — dex.universal_router)"),
    d("Контракты DEX", "dex.quoter", K::Address, L::None, "адрес", DX, "Квотер: QuoterV2 (V3), CLQuoter / BinQuoter (Infinity), V4Quoter (Uniswap V4)"),
    d("Контракты DEX", "dex.tick_lens", K::Address, L::None, "адрес", DX, "V3: TickLens (тики пачкой)"),
    d("Контракты DEX", "dex.pool_manager", K::Address, L::None, "адрес", DX, "V4: CLPoolManager / BinPoolManager / PoolManager — пул из справочника проверяется по ключу у менеджера (Infinity: poolIdToPoolKey)"),
    d("Контракты DEX", "dex.position_manager", K::Address, L::None, "адрес", DX, "Uniswap V4: PositionManager — ключ пула из справочника (poolKeys по PoolId) для проверки; пул без позиций через него не находится — тогда только история V4 (dex.history_scan)"),
    d("Контракты DEX", "dex.vault", K::Address, L::None, "адрес", DX, "PancakeSwap Infinity: Vault"),
    d("Контракты DEX", "dex.state_view", K::Address, L::None, "адрес", DX, "Uniswap V4: StateView (чтение состояния пула)"),
    d("Контракты DEX", "dex.universal_router", K::Address, L::None, "адрес", DX, "Universal Router этой DEX — прямой обмен в её пуле любого вида (решение 40: проверка баланса в той же транзакции). Universal Router знает только свою фабрику: адрес — с официальной страницы той же DEX. Не задан — пулы DEX не берутся («не подключена»): пул без роутера в сделках не участвует. Совпадает со smart.universal_router — пулы DEX идут и в умный маршрут"),
    d("Контракты DEX", "dex.universal_router_version", K::Choice, L::None, "версия", DX, "Формат команд Universal Router этой DEX: pancakeswap_infinity — Universal Router PancakeSwap Infinity; 2.0 / 2.1.1 / 2.1.2 — версии Universal Router Uniswap (официальный SDK)").choices(UR_FORMATS),
    d("Контракты DEX", "dex.permit2", K::Address, L::None, "адрес", DX, "Permit2 своей DEX (у PancakeSwap и Uniswap — разные)"),
    d("Контракты DEX", "dex.v2_fee_source", K::Choice, L::None, "источник", DX, "V2: откуда комиссия пары — fixed: одна на всю DEX (dex.v2_fee_pct, из кода пары своей DEX); pool: своя у каждой пары, читается с пары методом dex.v2_fee_method при проверке пула (например Biswap: swapFee())").choices(&["fixed", "pool"]),
    d("Контракты DEX", "dex.v2_fee_pct", K::Decimal, L::Percent, "%", DX, "V2 (dex.v2_fee_source = fixed): комиссия пары (из кода пары своей DEX)"),
    d("Контракты DEX", "dex.v2_fee_method", K::Text, L::None, "сигнатура", DX, "V2 (dex.v2_fee_source = pool): view-метод пары без аргументов, возвращающий комиссию целым, — сигнатура как в коде пары своей DEX (Biswap: swapFee() — BiswapPair.sol)"),
    d("Контракты DEX", "dex.v2_fee_denominator", K::Integer, L::Positive, "делитель", DX, "V2 (dex.v2_fee_source = pool): комиссия = значение метода / делитель (Biswap: 1000 — balance·1000 − amountIn·swapFee в BiswapPair.sol)"),
];
