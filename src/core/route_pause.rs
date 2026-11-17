//! Файл: core/route_pause.rs
//! Что делает: пауза направления сделки (signal.route_cooldown_s) — одна функция у двух мест: закрытие сделки (ledger.rs
//! finish_paused, как было) и решение 293 (ревью) — продажа на бирже отклонена (OrderRejected: биржей или правилами
//! ордера адаптера до отправки) и сделка ушла человеку
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
//! Задача t179 (решение 390): после удачной сделки (won) пауза не ставится — ledger.rs.
//! Кто вызывает: ledger.rs (finish_paused, won), sell_unsent.rs (alert), тесты (tests/core_sell_reject_293.rs).
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

/// Задача t179 (решение 390): строка хронологии удачной сделки — паузы направления нет.
pub const WON_NOTE: &str = "Пауза направления не поставлена: сделка удачная (продана на площадке плана, итог ≥ 0) — решение 390";

/// Что делает: задача t179 (решение 390) — сделка удачная, пауза направления после неё не нужна: закрыта (Closed),
/// была продажа, монеты проданы на площадке продажи направления без перегонов, итог ≥ 0. Срыв покупки, перевод не
/// состоялся, перегон, продажа вручную без продаж бота, убыток — не удачная (пауза как раньше).
/// Принимает: сделку (шаг и итог уже записаны finish). Ошибок нет.
pub fn won(deal: &Deal) -> bool {
    deal.step == super::deal::Step::Closed
        && !deal.sells.is_empty()
        && deal.relays == 0
        && deal.at == deal.direction.sell_on
        && deal
            .result_usd
            .is_some_and(|r| r >= rust_decimal::Decimal::ZERO)
}

/// Имя настройки «пауза направления после отказа биржи на продаже» (каталог — settings/catalog/trade.rs).
pub const REJECT_PAUSE: &str = "stuck.reject_route_pause";

/// Что делает: ставит паузу направления на signal.route_cooldown_s. Принимает: ядро, направление.
/// Возвращает: на сколько секунд поставлена пауза; None — не поставлена (ledger.rs результат не читает).
/// Ошибок нет: настройка не задана или неверна — паузы нет, запись Warn в журнал core.ledger.
pub(super) async fn pause(core: &Core, d: &Direction) -> Option<u64> {
    let deps = &core.inner.deps;
    match deps
        .settings
        .integer("signal.route_cooldown_s", At::default())
    {
        Ok(s) => {
            lk(&core.inner.signals).cooldown(d, s, Utc::now());
            Some(s)
        }
        Err(e) => {
            deps.log(Level::Warn, "core.ledger", None, e.message, None)
                .await;
            None
        }
    }
}

/// Что делает: решение 293 (ревью) — продажа отклонена (биржей или правилами ордера адаптера до отправки) и ушла
/// человеку: пауза направления сделки, если включена stuck.reject_route_pause; поставлена — запись Info в журнал
/// core.ledger (монета, направление, секунды). Принимает: ядро, сделку, ошибку продажи. Возвращает: true — пауза
/// поставлена (или попытка: signal.route_cooldown_s не задана — Warn, без Info). Ошибок нет.
pub async fn after_reject(core: &Core, deal: &Deal, e: &OpError) -> bool {
    if !rejected_on_cex(core, deal, e) || !enabled(core, &deal.at.0) {
        return false;
    }
    let d = &deal.direction;
    if let Some(secs) = pause(core, d).await {
        let m = format!(
            "{}: пауза направления {} {} → {} на {secs} с — продажа на {} отклонена (stuck.reject_route_pause, решение 293)",
            deal.id, d.asset.ticker, d.buy_on.0, d.sell_on.0, deal.at.0
        );
        core.inner
            .deps
            .log(Level::Info, "core.ledger", Some(&deal.at.0), m, None)
            .await;
    }
    true
}

/// Что делает: продажа отклонена — OrderRejected (ответ биржи или правила ордера адаптера до отправки, например
/// venues/bingx/order_rules.rs ready), монеты сделки на площадке вида «биржа» (Cex).
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
