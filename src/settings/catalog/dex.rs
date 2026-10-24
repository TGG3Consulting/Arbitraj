//! Файл: settings/catalog/dex.rs
//! Что делает: настройки адаптера DEX — узел BSC, режим обмена, пулы, проверка расчёта, разрешения,
//! умный режим и адреса контрактов каждого вида пула. Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, решения 29, 31, 38).
//!
//! Адреса контрактов — только из официальной документации DEX (research/flow/bsc_dex.md, bsc_v4.md,
//! bsc_smart_router.md); вводит Тигран. Область «DEX» (dx): id — DEX («pancakeswap») для dex.enabled
//! или вид пула («pancakeswap_v3») для остальных dex.* с этой областью.

use super::{DX, G, V, d};
use crate::settings::schema::{DexId, Kind as K, Limit as L, SettingDef};

/// Настройки DEX. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static DEX: &[SettingDef] = &[
    d("Узел BSC", "bsc.node_ws_url", K::Url, L::None, "URL", G, "WebSocket узла BSC (подписки logs, newHeads); ключ провайдера в адресе — как {key}, сам ключ — секрет bsc.node_ws_key"),
    d("Узел BSC", "bsc.node_http_rps", K::Integer, L::Positive, "вызовов в секунду", G, "Темп узла HTTP: сколько вызовов JSON-RPC в секунду (именно вызовов, не CU провайдера — значение подбирается под тариф); пачка из N вызовов — N; на «превышен лимит» (429) — правило шлюза «Превышен лимит запросов»"),
    d("Узел BSC", "bsc.node_http_url", K::Url, L::None, "URL", G, "HTTP узла BSC (eth_call, eth_getLogs, eth_estimateGas, квитанции); ключ — как {key}, секрет bsc.node_http_key"),
    d("DEX", "dex.swap_mode", K::Choice, L::None, "режим", V, "Режим обмена: прямой (лучший пул) / умный (маршрут через Universal Router) (решение 29)").choices(&["direct", "smart"]),
    d("DEX", "dex.enabled", K::Bool, L::None, "да/нет", DX, "DEX включена (id области — имя DEX, например pancakeswap)").dex_id(DexId::Dex),
    d("DEX", "dex.pool_enabled", K::Bool, L::None, "да/нет", DX, "Вид пула включён (id области — вид пула, например pancakeswap_v3) (решение 31)"),
    d("DEX", "dex.intermediate_enabled", K::Bool, L::None, "да/нет", V, "Промежуточные монеты (решение 52): да — маршрут «USDT → промежуточная → монета» в одной транзакции и монеты с пулом только с промежуточной допускаются; нет — только пулы с USDT"),
    d("DEX", "dex.inflow_wait_notify_s", K::Integer, L::Positive, "секунды", V, "Обмен ждёт ожидаемого поступления того же токена в кошелёк дольше — одно уведомление в Telegram (dex.swap_waits_inflow)"),
    d("DEX", "dex.native_ticker", K::Text, L::None, "тикер", V, "Как биржа называет нативную монету BSC (BNB): этим тикером записываются затраты шага через её обёртку (WETH() роутера V2 своей DEX, WBNB 1:1)"),
    d("DEX", "dex.intermediate_tokens", K::AddressList, L::None, "адреса BSC", V, "Промежуточные монеты (решение 52): адреса токенов в BSC (например WBNB). Нужен, если dex.intermediate_enabled = да"),
    d("DEX", "dex.min_pool_liquidity_usd", K::Decimal, L::NonNegative, "USDT", V, "Пулы с ликвидностью ниже не котируются, не сверяются с квотером и не участвуют в пробе налогов (фильтр — первым)"),
    d("DEX", "dex.pool_load_retries", K::Integer, L::Positive, "попыток", V, "Состояние пула не прочиталось при подключении (сбой узла): сколько раз повторить чтение — по разу на каждый новый блок; дальше пул недостоверен до переподключения подписки"),
    d("DEX", "dex.pools_per_sub", K::Integer, L::Positive, "штук", V, "Сколько пулов в одной подписке logs (узел BSC: до 1000 вариантов в позиции topics)"),
    d("DEX", "dex.logs_block_batch", K::Integer, L::Positive, "блоков", V, "Сколько блоков в одном eth_getLogs при чтении истории Initialize пулов V4 (не больше лимита провайдера узла)"),
    d("DEX", "dex.history_scan", K::Bool, L::None, "да/нет", V, "Читать историю пулов V4 (разовое чтение Initialize, eth_getLogs): нет — пауза перед следующим куском, место сохраняется, найденные пулы слушаются, виды пулов не выключаются"),
    d("DEX", "dex.scan_progress_s", K::Integer, L::Positive, "секунды", V, "Чтение истории пулов V4: ход (прочитано, осталось блоков) — в журнал не чаще раза в столько секунд"),
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
    d("Контракты DEX", "dex.factory", K::Address, L::None, "адрес", DX, "V2/V3: фабрика пулов"),
    d("Контракты DEX", "dex.router", K::Address, L::None, "адрес", DX, "V2: Router — сверка расчёта пула (getAmountsOut) и обёртка нативной монеты (WETH()); виды V3 и V4 его не используют (обмен — dex.universal_router)"),
    d("Контракты DEX", "dex.v3_fee_tiers", K::TextList, L::None, "уровни fee", DX, "V3: уровни комиссии для поиска пулов getPool — целые в миллионных долях (100 = 0,01%), через запятую; фабрика их не перечисляет, каждый сверяется с feeAmountTickSpacing фабрики"),
    d("Контракты DEX", "dex.quoter", K::Address, L::None, "адрес", DX, "Квотер: QuoterV2 (V3), CLQuoter / BinQuoter (Infinity), V4Quoter (Uniswap V4)"),
    d("Контракты DEX", "dex.tick_lens", K::Address, L::None, "адрес", DX, "V3: TickLens (тики пачкой)"),
    d("Контракты DEX", "dex.pool_manager", K::Address, L::None, "адрес", DX, "V4: CLPoolManager / BinPoolManager / PoolManager"),
    d("Контракты DEX", "dex.vault", K::Address, L::None, "адрес", DX, "PancakeSwap Infinity: Vault"),
    d("Контракты DEX", "dex.state_view", K::Address, L::None, "адрес", DX, "Uniswap V4: StateView (чтение состояния пула)"),
    d("Контракты DEX", "dex.universal_router", K::Address, L::None, "адрес", DX, "V4: Universal Router своей DEX — прямой обмен в пуле V4"),
    d("Контракты DEX", "dex.universal_router_version", K::Text, L::None, "версия", DX, "Версия Universal Router (от неё зависит формат обмена, у Uniswap: 2.0 / 2.1.1 / 2.1.2)"),
    d("Контракты DEX", "dex.permit2", K::Address, L::None, "адрес", DX, "Permit2 своей DEX (у PancakeSwap и Uniswap — разные)"),
    d("Контракты DEX", "dex.v2_fee_pct", K::Decimal, L::Percent, "%", DX, "V2: комиссия пула (из кода пары своей DEX)"),
];
