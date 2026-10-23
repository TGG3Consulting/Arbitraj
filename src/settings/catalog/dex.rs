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
    d("Узел BSC", "bsc.node_http_url", K::Url, L::None, "URL", G, "HTTP узла BSC (eth_call, eth_getLogs, eth_estimateGas, квитанции); ключ — как {key}, секрет bsc.node_http_key"),
    d("DEX", "dex.swap_mode", K::Choice, L::None, "режим", V, "Режим обмена: прямой (лучший пул) / умный (маршрут через Universal Router) (решение 29)").choices(&["direct", "smart"]),
    d("DEX", "dex.enabled", K::Bool, L::None, "да/нет", DX, "DEX включена (id области — имя DEX, например pancakeswap)").dex_id(DexId::Dex),
    d("DEX", "dex.pool_enabled", K::Bool, L::None, "да/нет", DX, "Вид пула включён (id области — вид пула, например pancakeswap_v3) (решение 31)"),
    d("DEX", "dex.min_pool_liquidity_usd", K::Decimal, L::NonNegative, "USDT", V, "Пулы с ликвидностью ниже не слушаются"),
    d("DEX", "dex.multicall", K::Address, L::None, "адрес", V, "Контракт Multicall для пачек eth_call (адрес — из официальной документации)"),
    d("DEX", "dex.pools_per_sub", K::Integer, L::Positive, "штук", V, "Сколько пулов в одной подписке logs (узел BSC: до 1000 вариантов в позиции topics)"),
    d("DEX", "dex.logs_block_batch", K::Integer, L::Positive, "блоков", V, "Сколько блоков в одном eth_getLogs при поиске пулов"),
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
    d("Умный режим", "smart.max_hops", K::Integer, L::Positive, "пулов", V, "Сколько пулов подряд в одной доле маршрута"),
    d("Умный режим", "smart.max_splits", K::Integer, L::Positive, "долей", V, "На сколько долей можно делить вход"),
    d("Умный режим", "smart.split_step_pct", K::Decimal, L::Percent, "%", V, "Шаг доли при поиске деления входа"),
    d("Контракты DEX", "dex.factory", K::Address, L::None, "адрес", DX, "V2/V3: фабрика пулов"),
    d("Контракты DEX", "dex.pool_deployer", K::Address, L::None, "адрес", DX, "PancakeSwap V3: PoolDeployer (по нему Universal Router вычисляет адрес пула)"),
    d("Контракты DEX", "dex.init_code_hash", K::Text, L::None, "хеш", DX, "V2/V3: init code hash пула (должен совпадать с зашитым в Universal Router)"),
    d("Контракты DEX", "dex.router", K::Address, L::None, "адрес", DX, "V2: Router; V3: SwapRouter — прямой обмен"),
    d("Контракты DEX", "dex.quoter", K::Address, L::None, "адрес", DX, "Квотер: QuoterV2 (V3), CLQuoter / BinQuoter (Infinity), V4Quoter (Uniswap V4)"),
    d("Контракты DEX", "dex.tick_lens", K::Address, L::None, "адрес", DX, "V3: TickLens (тики пачкой)"),
    d("Контракты DEX", "dex.pool_manager", K::Address, L::None, "адрес", DX, "V4: CLPoolManager / BinPoolManager / PoolManager"),
    d("Контракты DEX", "dex.vault", K::Address, L::None, "адрес", DX, "PancakeSwap Infinity: Vault"),
    d("Контракты DEX", "dex.state_view", K::Address, L::None, "адрес", DX, "Uniswap V4: StateView (чтение состояния пула)"),
    d("Контракты DEX", "dex.universal_router", K::Address, L::None, "адрес", DX, "V4: Universal Router своей DEX — прямой обмен в пуле V4"),
    d("Контракты DEX", "dex.universal_router_version", K::Text, L::None, "версия", DX, "Версия Universal Router (от неё зависит формат обмена, у Uniswap: 2.0 / 2.1.1 / 2.1.2)"),
    d("Контракты DEX", "dex.permit2", K::Address, L::None, "адрес", DX, "Permit2 своей DEX (у PancakeSwap и Uniswap — разные)"),
    d("Контракты DEX", "dex.scan_from_block", K::Integer, L::NonNegative, "блок", DX, "С какого блока искать события создания пулов (Initialize, PairCreated, PoolCreated)"),
    d("Контракты DEX", "dex.v2_fee_pct", K::Decimal, L::Percent, "%", DX, "V2: комиссия пула (из кода пары своей DEX)"),
];
