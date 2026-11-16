//! Файл: settings/catalog/dex_par.rs
//! Что делает: настройки задачи t153 (16.11.2026) — одновременное чтение пулов вместо чтения друг за другом:
//! решение 331 — пулы маршрута перед сделкой и цикла DEX ↔ DEX (verify.read_parallel_max). Раздел «DEX», область
//! «площадка», только площадка DEX — .dex_venue(). Необязательные, значений нет: не задана — как раньше, по очереди.
//! Отдельный файл: dex_event.rs и dex.rs — зоны других задач.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «DEX»).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 331 (t153). Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static DEX_PAR: &[SettingDef] = &[
    d("DEX", "verify.read_parallel_max", K::Integer, L::Positive, "чтений", V, "Решение 331: перечтение пулов маршрута перед сделкой (предторговая верификация, решение 83) и ног цикла DEX ↔ DEX (свежее состояние перед сделкой и сухой сделкой): пулы видов «по одному» (Algebra, Algebra Integral, Slipstream, Infinity Bin, LB, DODO, KyberSwap Elastic / DMM, стейблсвапы, WooFi, Maverick, Metric, iZi, Biswap V3, SmarDex, Balancer, Solidly) и пары многомонетных пулов читаются одновременно — не больше столько чтений сразу — и одновременно со ступенями Multicall3 пулов V2 / V3 / CL / V4; раньше — друг за другом, а ступени ждали их всех. Тот же блок, те же функции чтения, тот же итог; запросы — очередью узла bsc.node_http_rps классом пути сделки (trade). Время перечтения ограничено verify.timeout_ms, как раньше. Не задана — как раньше, по одному. Необязательна").dex_venue().optional(),
];
