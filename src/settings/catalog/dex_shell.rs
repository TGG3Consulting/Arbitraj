//! Файл: settings/catalog/dex_shell.rs
//! Что делает: настройки вида пулов Shell Protocol Proteus (решение 486, dex.kind = shell_proteus): списки сверенных
//! реализаций формулы Proteus по виду формулы полезности. У Shell реализация пула — внешний контракт за прокси
//! (implementation() пула, владелец может сменить); развёрнутые реализации различаются формулой полезности (_getUtility
//! — две формулы) и BASE_FEE (читается с реализации). Пул с реализацией не из списков не считается (расчёт был бы
//! вслепую). Учёт Ocean — dex.vault, пулы — dex.stable_pools (каталог dex.rs).
//! Отдельный файл: каталог dex.rs правят параллельно другие семейства. Область — «DEX» (DX). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, группа «Контракты DEX»).
//!
//! Факты (решение 486, сверка 20.11.2026): исходник Sourcify 42161 (exact_match) каждой реализации пулов из
//! docs.shellprotocol.io/start-here/reference-library/contract-addresses сравнён с Shell-Protocol/Shell-Protocol
//! (git-блобы): V1 (bfbba01 Proteus.sol) — 0x537e2d22cc140c15ec03e99be399223750de7737 (DAI+USDC),
//! 0xd5a7edb913eae552e341b3ba0704cb2f6971bf7b (USDT+USDC), 0xb827ce2b24518eaa13663aff541632c8cb3e8c71 (Stablepool,
//! BASE_FEE 8000); V2 (квадратное уравнение) — 0x243e83365063baa4feda9d71c872dbabe035280b (ARB+ETH),
//! 0xa70615e0aa71f83253d37d19282e6bac7f42ce07 (TOUCOIN+ETH), 0xcd27114b23a7b913bc778994ae2996897a6d950b (MAGIC+ETH),
//! 0xdfd421298f09555e32611ac00d1dd3b22a583ebf (Smolpool), 0x0365d47adfbacc8050dc5a39f18687ba41d092b6 (REUNI+ETH),
//! 0x9b41d5991b20e2f5eb3dbae6d4c4f65f54981ccb (STG+ETH), 0xe55f3d89043154563cec9b283fa57752442bc56a (WBTC+USD) — Ocean
//! 0xC32e…3584; 0xf6e9c132fa6f5102ed137014a4512bfc7696c92e (SHELL+ETH, Ocean 0x96B4…fB7a). Вводит Тигран.

use super::{DX, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 486. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static DEX_SHELL: &[SettingDef] = &[
    d("Контракты DEX", "dex.proteus_utility_v1", K::AddressList, L::None, "адреса", DX, "Решение 486: Shell Protocol (shell_proteus) — реализации Proteus с формулой полезности V1 (u = (bx + ay ± √((bx)² + 4xy − 2aybx + (ay)²)) / k(2 − 2ab), Shell-Protocol bfbba01 Proteus.sol L547-L568): адреса implementation() пулов, исходник которых на Sourcify побайтно сверен с этой формулой. Пул, чья реализация не в этом списке и не в dex.proteus_utility_v2, не считается (причина в журнале); смена реализации (событие ImplementationChanged) — пул перечитывается и сверяется снова. Не задана — пулы с такими реализациями не считаются"),
    d("Контракты DEX", "dex.proteus_utility_v2", K::AddressList, L::None, "адреса", DX, "Решение 486: Shell Protocol (shell_proteus) — реализации Proteus с формулой полезности V2 (квадратное уравнение k(ab − 1)u² + (ay + bx)u + xy/k = 0 в масштабе 1e18; при a < 0 и b < 0 — меньший корень; Sourcify 42161 реализации 0x243e8336… Proteus.sol L551-L570): адреса implementation() пулов, исходник которых на Sourcify сверен с этой формулой. Не задана — пулы с такими реализациями не считаются"),
];
