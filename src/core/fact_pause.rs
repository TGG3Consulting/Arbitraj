//! Файл: core/fact_pause.rs
//! Что делает: решение 413 (третье ревью t198, С-Д) — пока баланс биржи не читается (после повторов по правилу
//! «повторить»), новых входов на эту биржу нет: пауза площадки существующим механизмом софтовых пауз (Risk.venue_paused
//! с отметкой auto_pause::Kind::NoFact, решение 225) с причиной «баланс биржи не читается: …». Снимает её само первое
//! успешное чтение баланса этой биржи (remain_read.rs), только если отметка всё ещё NoFact (паузу человека или другого
//! модуля не трогает). Журнал core.control, Telegram risk.stop (постановка) — как у других софтовых пауз. Не торгуем
//! вслепую.
//! Кто вызывает: core/remain_read.rs (set, lift).
//! От чего зависит: auto_pause.rs (pause_venue, lift_venue), risk.rs (publish), engine.rs, deps.rs.
//! Настройки: нет.

use crate::core::types::VenueId;
use crate::storage::journal::Level;

use super::auto_pause::Kind;
use super::engine::{Core, lk};

/// Что делает: пауза биржи «баланс не читается» (если её ещё нет). Принимает: ядро, биржу, причину. Ошибок нет.
pub async fn set(core: &Core, v: &VenueId, why: &str) {
    let text = format!("баланс биржи не читается: {why}");
    let fresh = {
        let mut r = lk(&core.inner.risk);
        let already = r.venue_auto.get(v).is_some_and(|m| m.kind == Kind::NoFact);
        !already
            && super::auto_pause::pause_venue(
                &mut r,
                v,
                text.clone(),
                Kind::NoFact,
                chrono::Utc::now(),
            )
    };
    if !fresh {
        return;
    }
    super::risk::publish(core);
    let deps = &core.inner.deps;
    let msg = format!(
        "Пауза площадки {} — {text}. Новые сделки с ней не начинаются; снимется сама при первом успешном чтении \
         баланса (решение 413)",
        v.0
    );
    deps.log(Level::Warn, "core.control", Some(&v.0), &msg, None)
        .await;
    let acts = vec![crate::notify::acts::resume_venue(&v.0)];
    deps.notify_acts("risk.stop", Some(&v.0), &msg, acts).await;
}

/// Что делает: баланс биржи прочитан — снять паузу «баланс не читается», если отметка та же. Ошибок нет.
pub async fn lift(core: &Core, v: &VenueId) {
    let lifted = {
        let mut r = lk(&core.inner.risk);
        match r.venue_auto.get(v).cloned() {
            Some(m) if m.kind == Kind::NoFact => super::auto_pause::lift_venue(&mut r, v, &m),
            _ => false,
        }
    };
    if !lifted {
        return;
    }
    super::risk::publish(core);
    let msg = format!(
        "Пауза площадки {} снята: баланс биржи снова читается (решение 413)",
        v.0
    );
    core.inner
        .deps
        .log(Level::Info, "core.control", Some(&v.0), msg, None)
        .await;
}
