//! Файл: core/route_pause.rs
//! Что делает: пауза направления сделки (signal.route_cooldown_s) — одна функция для двух мест: закрытие сделки
//! (ledger.rs finish_paused, как было) и решение 293 (ревью): продажа на БИРЖЕ отклонена (OrderRejected) и сделка ушла
//! человеку (sell_unsent.rs alert) — чтобы следующие сделки той же монеты в том же направлении не покупали её снова,
//! пока человек разбирается (живого случая нет: риск, если биржа соблюдает пределы, которые бот не соблюдает).
//! Не задана signal.route_cooldown_s — паузы нет, запись Warn в журнал core.ledger (так было в ledger.rs).
//! Где пауза действует: core/signal.rs Signals::update — состояние Cooldown вместо Candidate, пока не истекла; новые
//! сделки начинаются только из Candidate (core/evaluate.rs → candidates::enqueue). Ручные действия по сделке («Продать
//! сейчас», «Продать и перегнать», «Беру на себя» — manual*.rs) паузу не читают.
//! Кто вызывает: ledger.rs (finish_paused), sell_unsent.rs (alert).
//! От чего зависит: signal.rs (Signals::cooldown), engine.rs, settings, deps.rs (журнал).
//! Настройки: signal.route_cooldown_s.

use chrono::Utc;

use crate::core::types::Direction;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk};

/// Что делает: ставит паузу направления на signal.route_cooldown_s. Принимает: ядро, направление.
/// Возвращает: ничего. Ошибок нет: настройка не задана или неверна — паузы нет, запись Warn в журнал core.ledger.
pub(super) async fn pause(core: &Core, d: &Direction) {
    let deps = &core.inner.deps;
    match deps
        .settings
        .integer("signal.route_cooldown_s", At::default())
    {
        Ok(s) => lk(&core.inner.signals).cooldown(d, s, Utc::now()),
        Err(e) => {
            deps.log(Level::Warn, "core.ledger", None, e.message, None)
                .await
        }
    }
}
