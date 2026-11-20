//! Файл: settings/catalog/dex_bv3_buffer.rs
//! Что делает: настройки решения 474 — обмен Balancer V3 через буфер ERC4626 хранилища (boosted-пулы: токен пула —
//! обёртка, монета пары — её базовый токен): опорная сумма превью обёртки и адрес BatchRouter V3 для квотера сверки.
//! Отдельный файл: каталог dex.rs правят параллельно другие семейства. Область — «DEX» (DX). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, группа «Контракты DEX»).
//!
//! Адрес BatchRouter — balancer/balancer-deployments@b55dc2b8 v3/tasks/20241205-v3-batch-router/output/arbitrum.json
//! (Arbitrum One — 0xaD89051bEd8d96f045E8912aE1672c6C0bF8a85E). Вводит Тигран.

use super::{DX, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 474. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static DEX_BV3_BUFFER: &[SettingDef] = &[
    d("Контракты DEX", "dex.bv3_buffer_preview_raw", K::Text, L::None, "сырые единицы", DX, "Решение 474: Balancer V3 (balancer_v3) — опорная сумма превью буфера ERC4626 (целое число в сырых единицах токена, десятичной записью, например 1000000000000000000000000000000). На каждом блоке бот читает у каждой обёртки пула previewDeposit / previewRedeem этой суммы и считает обмен через буфер как ⌊сумма · превью(опорная) / опорная⌋ (math/balancer_v3_buffer.rs: превью — своя функция каждой обёртки, по одной точке; чем больше опорная сумма относительно сделки, тем меньше расхождение — не больше 1 единицы токена у границы целого). Задана — пары пулов, где монета — базовый токен обёртки с инициализированным буфером хранилища, котируются (WRAP → пул → UNWRAP) и исполняются шагом BALANCER_V3_BUF = 27 (нужна сборка исполнителя v5). Не задана — обмена через буфер нет (пулы с обёртками — как раньше, парами обёрток); не число — пул не читается с причиной").optional(),
    d("Контракты DEX", "dex.bv3_batch_router", K::Address, L::None, "адрес", DX, "Решение 474: Balancer V3 (balancer_v3) — BatchRouter V3: квотер сверки пар через буфер ERC4626 — querySwapExactIn с путём «буфер → пул → буфер» (шаги isBuffer; eth_call без from — Vault.quote требует tx.origin = 0). Arbitrum — 0xaD89051bEd8d96f045E8912aE1672c6C0bF8a85E (balancer-deployments@b55dc2b8 v3/tasks/20241205-v3-batch-router/output/arbitrum.json). Не задан — пары через буфер без квотера сверки (котировка только расчётом бота; сверка перед сделкой — ошибка «не задана настройка»)").optional(),
];
