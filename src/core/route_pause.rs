//! Файл: core/route_pause.rs
//! Что делает: пауза направления сделки (signal.route_cooldown_s) — одна функция у двух мест: закрытие сделки (ledger.rs
//! finish_paused, как было) и решение 293 (ревью) — продажа на БИРЖЕ отклонена (OrderRejected) и сделка ушла человеку
//! (sell_unsent.rs alert → after_reject), только при необязательной stuck.reject_route_pause = да (область — система
//! или площадка, где отклонена продажа). Не задана или «нет» — как раньше: паузы нет, записей в журнале нет.
//! Что именно делает пауза (факт кода): ровно одно направление — монета, площадка покупки, площадка продажи сделки
//! (Direction, core/signal.rs Signals::cooldown); на signal.route_cooldown_s секунд; только в памяти (после перезапуска
//! её нет). Обратное направление, другие пары площадок этой монеты и другие монеты не останавливаются. Не задана
//! signal.route_cooldown_s — паузы нет, Warn в журнал core.ledger (как было в ledger.rs).
//! Где пауза действует: core/signal.rs Signals::update — State::Cooldown вместо Candidate, пока не истекла; новые сделки
//! начинаются только из Candidate (core/evaluate.rs → candidates::enqueue). Ручные действия по сделке («Продать
//! сейчас», «Продать и перегнать», «Беру на себя» — manual*.rs) паузу не читают. Продажа кнопкой человека, отклонённая
//! биржей, тоже ставит паузу (sell_unsent::decide: кнопка и отказ не «до отправки» → Retry::No → sell.rs → alert).
//! Кто вызывает: ledger.rs (finish_paused), sell_unsent.rs (alert), тесты (tests/core_sell_reject_293.rs).
//! От чего зависит: signal.rs (Signals::cooldown), trade_ops.rs (вид площадки), engine.rs, settings, deps.rs (журнал).
//! Настройки: signal.route_cooldown_s, stuck.reject_route_pause.

use chrono::Utc;

use crate::core::types::Direction;
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::{Core, lk};
use super::op_error::OpError;

/// Имя настройки «пауза направления после отказа биржи на продаже» (каталог — settings/catalog/trade.rs).
pub const REJECT_PAUSE: &str = "stuck.reject_route_pause";

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

/// Что делает: решение 293 (ревью) — продажа отклонена биржей и ушла человеку: пауза направления сделки, если включена
/// stuck.reject_route_pause. Принимает: ядро, сделку, ошибку продажи. Возвращает: true — пауза поставлена (или
/// попытка: signal.route_cooldown_s не задана — Warn). Ошибок нет.
pub async fn after_reject(core: &Core, deal: &Deal, e: &OpError) -> bool {
    if !rejected_on_cex(core, deal, e) || !enabled(core, &deal.at.0) {
        return false;
    }
    pause(core, &deal.direction).await;
    true
}

/// Что делает: продажа отклонена биржей — OrderRejected, монеты сделки на площадке вида «биржа» (Cex).
/// Принимает: ядро, сделку, ошибку. Возвращает: да/нет. Ошибок нет (площадки нет в реестре — нет).
pub fn rejected_on_cex(core: &Core, deal: &Deal, e: &OpError) -> bool {
    e.code == Some(ErrorCode::OrderRejected)
        && matches!(super::trade_ops::kind(core, &deal.at), Ok(VenueKind::Cex))
}

/// Что делает: stuck.reject_route_pause для площадки (значение площадки сильнее системы). Принимает: ядро, площадку.
/// Возвращает: true — «да»; не задана, «нет» или неверный вид — false (без записей в журнал). Ошибок нет.
fn enabled(core: &Core, venue: &str) -> bool {
    let at = At {
        venue: Some(venue),
        coin: None,
    };
    core.inner
        .deps
        .settings
        .flag(REJECT_PAUSE, at)
        .unwrap_or(false)
}
