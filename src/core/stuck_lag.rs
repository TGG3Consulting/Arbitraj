//! Файл: core/stuck_lag.rs
//! Что делает: решение 334 (задача t155) — ожидание тика монеты застрявшей сделки (stuck.rs next_tick: режим
//! «Ожидание», «Перегон» — relay_here.rs) отстало от рассылки тиков ядра (broadcast coin_ticks, ёмкость --live-buffer;
//! RecvError::Lagged — пропущено n тиков, среди них мог быть тик ЭТОЙ монеты). Раньше — молча ждали следующего тика
//! монеты (у тихой монеты — до срока stuck.wait_ms, а проверка «стало выгодно» пропускалась).
//! - Всегда: счёт и запись журнала core.coin_ticks_lost (gateway/lost_log.rs: на 1-ю, 2-ю, 4-ю… или по интервалу).
//! - stuck.tick_lag_wake = да: отставание считается возможным тиком монеты — проверка выгодности сразу (лишняя
//!   проверка безвредна: она та же, что на настоящий тик). Не задана / нет — как раньше ждём следующего тика.
//!
//! Кто вызывает: core/stuck.rs (next_tick).
//! От чего зависит: engine.rs, deal.rs, gateway/lost_log.rs, settings.
//! Настройки: stuck.tick_lag_wake (вся система; необязательная); журнал — log.events_lost_interval_s.

use std::sync::LazyLock;

use crate::gateway::lost_log::{Lost, Note};
use crate::settings::read::At;

use super::deal::Deal;
use super::engine::Core;

/// Настройка «отставание — проверить сразу».
pub const WAKE: &str = "stuck.tick_lag_wake";
/// Вид записи журнала.
pub const LOST_KIND: &str = "core.coin_ticks_lost";

/// Счёт отставаний (один на процесс: рассылка тиков одна).
static LOST: LazyLock<Lost> = LazyLock::new(Lost::default);

/// Отставание ожидания тика: счёт и журнал; решает, проверять ли сразу. Принимает: ядро, сделку, сколько тиков
/// пропущено. Возвращает: true — проверить сразу (stuck.tick_lag_wake = да), false — ждать дальше. Ошибок нет.
pub fn lagged(core: &Core, deal: &Deal, n: u64) -> bool {
    let deps = &core.inner.deps;
    let note = Note {
        kind: LOST_KIND,
        venue: None,
        what: "ожидание тика монеты застрявшей сделки отстало от рассылки тиков ядра — тик монеты мог пропасть",
        last: format!(
            "{} (сделка {}), пропущено тиков {n}",
            deal.direction.asset.ticker, deal.id
        ),
    };
    LOST.hit(&deps.settings, &deps.journal, note);
    matches!(deps.settings.flag(WAKE, At::default()), Ok(true))
}
