//! Файл: core/withdraw_limit_pause.rs
//! Что делает: решение 253, дополнение t85 (Тигран 11.11: «ставить биржу на паузу и отправить алерт») — вывод отклонён,
//! потому что исчерпан лимит вывода АККАУНТА, который сбрасывается сам (код шлюза WithdrawLimit: суточный / месячный
//! лимит по официальной таблице ошибок биржи — venues/<биржа>/errors.rs). Следующая сделка с выводом с этой биржи до
//! сброса получила бы тот же отказ, поэтому:
//! - площадка — на паузу существующим механизмом (Risk.venue_paused, auto_pause::pause_venue с отметкой
//!   Kind::WithdrawLimit): новые сделки с её участием не открываются (risk::can_open), она не цель перегона
//!   (relay.rs, manual_relay_targets.rs); идущие сделки не трогаются — продажа и возврат уже купленного на ней
//!   паузу не проверяют (relay_here.rs, sell.rs, trade_ops.rs);
//! - снятие — только человек («Снять паузу» на экране «Управление», risk::resume_venue): время сброса лимита биржа не
//!   сообщает, поэтому автоснятия (auto_resume.rs), снятия новым днём (day_roll.rs) и сверкой нет;
//! - монета НЕ закрывается (withdraw_block.rs: дело не в монете);
//! - алерт venue.withdraw_limit шаблоном решения 251: «Лимит вывода <биржа> исчерпан: <причина> — биржа на паузе,
//!   снимите паузу после сброса лимита» (только когда пауза новая; повторный отказ — только журнал); уходит в чат роли
//!   «Аккаунты» (notify.accounts_chat), если он задан, иначе в «Общий» (notify/routes.rs).
//!   Ручная команда человека (вывод из «Продать и перегнать», «Перевести USDT») паузу тоже ставит: лимит — факт аккаунта,
//!   а не ошибка команды.
//!
//! Кто вызывает: core/rules_apply.rs (on_error — все выводы: сделка, перегон, возврат USDT, ручной перевод).
//! От чего зависит: auto_pause.rs, risk.rs, engine.rs, deps.rs, notify (human, human_msg, routes).
//! Настройки: нет своих (чат — notify.accounts_chat, notify/roles.rs).

use chrono::Utc;

use crate::core::types::VenueId;
use crate::gateway::errors::GatewayError;
use crate::notify::routes::Tag;
use crate::storage::journal::Level;

use super::auto_pause::{self as ap, Kind};
use super::engine::{Core, lk};

/// Код события уведомлений (notify/catalog.rs).
pub const EVENT: &str = "venue.withdraw_limit";

/// Что делает: пауза площадки по лимиту вывода, журнал core.withdraw_limit и (если пауза новая) алерт. Принимает:
/// ядро, что делали («Вывод»), площадку, id сделки (или None), ошибку WithdrawLimit. Ошибок нет.
pub async fn pause(core: &Core, op: &str, venue: &VenueId, deal: Option<&str>, err: &GatewayError) {
    let name = crate::notify::human::venue(&venue.0);
    let why = format!("лимит вывода исчерпан — {}", err.message);
    let fresh = {
        let mut r = lk(&core.inner.risk);
        ap::pause_venue(&mut r, venue, why.clone(), Kind::WithdrawLimit, Utc::now())
    };
    super::risk::publish(core);
    let deps = &core.inner.deps;
    let log = format!(
        "{op} на {}: {why} — площадка на паузе до ручного снятия, монета не закрыта{}",
        venue.0,
        if fresh {
            ""
        } else {
            " (пауза уже стояла)"
        }
    );
    deps.log(
        Level::Warn,
        "core.withdraw_limit",
        Some(&venue.0),
        &log,
        Some(serde_json::json!({ "deal": deal })),
    )
    .await;
    if !fresh {
        return;
    }
    let text = crate::notify::human_msg::Msg::event(EVENT)
        .line(format!("Лимит вывода {name} исчерпан"))
        .reason(&err.message)
        .bot(&format!(
            "{name} на паузе: новые сделки с ней не открываются; идущие сделки продолжаются; монета не закрыта"
        ))
        .todo(format!(
            "снимите паузу {name} («Снять паузу» на экране «Управление») после сброса лимита — время сброса биржа не сообщает"
        ))
        .id(deal.unwrap_or_default())
        .render();
    deps.notify_tagged(EVENT, Some(&venue.0), &text, Tag::Plain)
        .await;
}
