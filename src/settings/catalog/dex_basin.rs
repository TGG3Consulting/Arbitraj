//! Файл: settings/catalog/dex_basin.rs
//! Что делает: настройки вида пулов Basin (решение 485, dex.kind = basin_well; DEX Basin на Arbitrum и любые DEX того
//! же семейства): допущенные реализации скважин, функции скважин ConstantProduct2 и Stable2, таблица Stable2 с A.
//! Aquifer — общая настройка dex.factory. Отдельный файл: каталог dex.rs правят параллельно другие семейства. Область —
//! «DEX» (DX). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, группа «Контракты DEX»).
//!
//! Адреса — официальные: docs.basin.exchange/resources/contracts (раздел Arbitrum), проверены в Sourcify 42161
//! (Aquifer, Well Implementation, Upgradable Well, ConstantProduct2, Stable2 LUT — exact_match; Stable2 — match).
//! Вводит Тигран.

use super::{DX, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 485. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static DEX_BASIN: &[SettingDef] = &[
    d("Контракты DEX", "dex.basin_well_impls", K::AddressList, L::None, "адреса", DX, "Решение 485: Basin (basin_well) — допущенные реализации скважин: скважина берётся, если Aquifer (dex.factory) выпустил её с одной из них (wellImplementation(скважина)) или она — прокси ERC-1967 на такую скважину (так развёрнуты скважины Beanstalk на Arbitrum: ERC1967Proxy на клон Upgradable Well). Arbitrum (docs.basin.exchange/resources/contracts): Well Implementation 0xBA5106bd62b342afAcB93f1078fe60177A62d1a9, Upgradable Well 0xBA510995783111be5301d93CCfD5dE4e3B28e50B. Не задана — DEX не подключена"),
    d("Контракты DEX", "dex.basin_cp2", K::AddressList, L::None, "адреса", DX, "Решение 485: Basin — допущенные адреса функции скважины ConstantProduct2 (x·y = s², своей комиссии нет): скважина с другой функцией не берётся (функция проверяется при каждом чтении). Arbitrum — 0xBA15000450Bf6d48ec50BD6327A9403E401b72b4 (docs.basin.exchange); у ReseedBean.sol Beanstalk (6db8c07c) — 0xbA1500c28C8965521f47F17Fc21A7829D6E1343e (тот же расчёт обмена, Sourcify exact_match) — какой у скважин, покажет запись узла (tools/arbitrum_record/basin.py). Не задана — скважины ConstantProduct2 не берутся (нужна хотя бы одна из dex.basin_cp2 / dex.basin_stable2)").optional(),
    d("Контракты DEX", "dex.basin_stable2", K::AddressList, L::None, "адреса", DX, "Решение 485: Basin — допущенные адреса функции скважины Stable2 (стейблсвап из двух токенов, A из таблицы dex.basin_stable2_lut). Arbitrum — 0xba150052e11591D0648b17A0E608511874921CBC (docs.basin.exchange). Не задана — скважины Stable2 не берутся (нужна хотя бы одна из dex.basin_cp2 / dex.basin_stable2)").optional(),
    d("Контракты DEX", "dex.basin_stable2_lut", K::Address, L::None, "адрес", DX, "Решение 485: Basin — таблица Stable2 (LUT): её getAParameter() — параметр A функции Stable2 (конструктор Stable2 берёт A из таблицы, геттера A у функции нет). Соответствие проверяется: calcLpTokenSupply функции на резервах скважины = наш расчёт с этим A. Arbitrum — «Stable 2 Lookup Table with A = 1» 0xBA51055dAD14d3920e1798D2e8A152d91CaDb461 (docs.basin.exchange). Не задана — скважины Stable2 не берутся").optional(),
];
