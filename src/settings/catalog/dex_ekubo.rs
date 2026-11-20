//! Файл: settings/catalog/dex_ekubo.rs
//! Что делает: настройки вида пулов Ekubo (решение 447, dex.kind = ekubo; DEX ekubo-v3-arbitrum и любые DEX того же
//! семейства): адреса ядра Core, реестра ключей PoolKeyIndex, расширения MEVCapture и сколько слов битмапа тиков читать.
//! Отдельный файл: каталог dex.rs правят параллельно другие семейства. Область — «DEX» (DX). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, группа «Контракты DEX»).
//!
//! Адреса — официальные: docs.ekubo.org/reference/contracts/evm-v3 (Core, MEVCapture, MEVCaptureRouter — один адрес во
//! всех сетях, Arbitrum — Live); PoolKeyIndex — EkuboProtocol/evm-contracts broadcast/DeployAll.s.sol/42161
//! (run-1785724793522.json, commit 02e0d82f). Вводит Тигран.

use super::{DX, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 447. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static DEX_EKUBO: &[SettingDef] = &[
    d("Контракты DEX", "dex.ekubo_core", K::Address, L::None, "адрес", DX, "Решение 447: Ekubo (ekubo) — ядро Core: в нём все пулы (состояние, тики, битмап читаются sload ядра), события пулов (анонимный лог обмена и PositionUpdated) и обмен (шаг EKUBO исполнителя: lock → swap). Официальный адрес (docs.ekubo.org evm-v3, все сети, Arbitrum) — 0x00000000000014aA86C5d3c41765bb24e11bd701. Не задан — DEX не подключена"),
    d("Контракты DEX", "dex.ekubo_key_index", K::Address, L::None, "адрес", DX, "Решение 447: Ekubo — реестр ключей пулов PoolKeyIndex: ключ пула (token0, token1, config) по PoolId из справочника индексатора (poolKeyById); ключ должен дать тот же PoolId. Пул, которого нет в реестре, не берётся (причина на экране). Arbitrum — 0x898956fc2aed01d5f81f556ff5dcb10534285718 (EkuboProtocol/evm-contracts broadcast DeployAll.s.sol/42161, commit 02e0d82f; на странице контрактов docs.ekubo.org адреса нет). Не задан — DEX не подключена"),
    d("Контракты DEX", "dex.ekubo_mev_capture", K::Address, L::None, "адрес", DX, "Решение 447: Ekubo — расширение MEVCapture: пулы с этим расширением берутся (обмен — через forward ядра к расширению, расчёт — с доплатой за пройденные тики, math/ekubo_mev.rs). Официальный адрес (docs.ekubo.org evm-v3) — 0x5555fF9Ff2757500BF4EE020DcfD0210CFfa41Be. Не задан — пулы с MEVCapture не берутся (пулы без расширения — берутся). Пулы с другими расширениями (TWAMM, Oracle, BoostedFees) не берутся всегда").optional(),
    d("Контракты DEX", "dex.ekubo_bitmap_words", K::Integer, L::Positive, "слов", DX, "Решение 447: Ekubo — сколько слов битмапа тиков читать по каждую сторону от слова текущего тика (слово — 256 шагов тика пула). Расчёт обмена за прочитанными словами не идёт — он частичный (больше слов — глубже расчёт и больше работы узла на перечитывании). Не задана — DEX не подключена"),
];
