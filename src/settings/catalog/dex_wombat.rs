//! Файл: settings/catalog/dex_wombat.rs
//! Что делает: настройки вида Wombat Exchange (решение 484; wombat — пулы Wombat на Arbitrum One): реализации пула
//! (прокси EIP-1967), сверенные с исходником вида, — обычные (HighCovRatioFeePoolV2) и динамические (DynamicPoolV2).
//! Отдельный файл: каталог dex.rs общий для параллельных задач семейств DEX (решение 433). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Контракты DEX»).

use super::{DX, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 484. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static DEX_WOMBAT: &[SettingDef] = &[
    d("Контракты DEX", "dex.wombat_implementations", K::AddressList, L::None, "адреса", DX, "Решение 484: Wombat Exchange (wombat — пулы Wombat, id DEX wombat-exchange сети Arbitrum) — адреса реализаций HighCovRatioFeePoolV2, чей верифицированный код (Sourcify) сверен с исходником вида (wombat-exchange/v1-core 2ac067b8: путь обмена тот же — docs/sources/wombat_arbitrum/README.md). Пул — прокси EIP-1967, владелец может сменить реализацию; пул читается и берётся, только если реализация (слот прокси на блоке чтения) — в этом списке или в dex.wombat_dynamic_implementations (pools/load_wombat.rs kind_of). Где взять: docs/FIRST_RUN.md (адреса из Sourcify proxyResolution пулов docs.wombat.exchange) и запись tools/arbitrum_record/wombat.py (реализация каждого пула). Не задана — DEX не подключена"),
    d("Контракты DEX", "dex.wombat_dynamic_implementations", K::AddressList, L::None, "адреса", DX, "Решение 484: Wombat Exchange (wombat) — адреса реализаций DynamicPoolV2 (динамический пул: множитель _quoteFactor = цена актива входа / цена актива выхода, getRelativePrice() активов на блоке чтения), чей верифицированный код (Sourcify) сверен с исходником вида (wombat-exchange/v1-core 2ac067b8 pool/DynamicPoolV2.sol). Пул с реализацией из этого списка считается динамическим. Где взять: docs/FIRST_RUN.md и запись tools/arbitrum_record/wombat.py. Необязательная. Не задана — динамические пулы (frxETH, wstETH, ankrETH, jUSDC на Arbitrum) не читаются (причина — «реализация не сверена»), остальные пулы — как обычно").optional(),
];
