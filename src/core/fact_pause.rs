//! Файл: core/fact_pause.rs
//! Что делает: решение 413 (третье ревью t198, С-Д) — пока баланс биржи не читается (после повторов по правилу
//! «повторить»), новых входов на эту биржу нет: пауза площадки существующим механизмом софтовых пауз (Risk.venue_paused
//! с отметкой auto_pause::Kind::NoFact, решение 225) с причиной «баланс биржи не читается: …». Снимает её само первое
//! успешное чтение баланса этой биржи (remain_read.rs), только если отметка всё ещё NoFact (паузу человека или другого
//! модуля не трогает). Журнал core.control, Telegram risk.stop (постановка) — как у других софтовых пауз. Не торгуем
//! вслепую.
//! Четвёртое ревью t198: снимает и успешное чтение балансов круга сверки (reconcile.rs, С-1), и круг автоснятия
//! (auto_resume.rs → recheck: балансы счетов площадки прочитаны); поверх чужой паузы (сверка, человек, другой модуль)
//! не ставится — только журнал (С-6).
//! Кто вызывает: core/remain_read.rs (set, lift), core/reconcile.rs (lift), core/auto_resume.rs (recheck).
//! От чего зависит: auto_pause.rs (pause_venue, lift_venue), risk.rs (publish), engine.rs, deps.rs.
//! Настройки: нет.

use crate::core::types::VenueId;
use crate::storage::journal::Level;

use super::auto_pause::Kind;
use super::engine::{Core, lk};

/// Что делает: пауза биржи «баланс не читается» (если её ещё нет). Принимает: ядро, биржу, причину. Ошибок нет.
pub async fn set(core: &Core, v: &VenueId, why: &str) {
    let text = format!("баланс биржи не читается: {why}");
    // Четвёртое ревью (С-6): площадка уже на паузе (сверка 221, человек, другой модуль) — чужую паузу не трогаем (не
    // превращаем в NoFact и потом не снимаем): только журнал.
    let (fresh, other) = {
        let mut r = lk(&core.inner.risk);
        if r.venue_paused.contains_key(v) {
            let ours = r.venue_auto.get(v).is_some_and(|m| m.kind == Kind::NoFact);
            (false, !ours)
        } else {
            let now = chrono::Utc::now();
            (
                super::auto_pause::pause_venue(&mut r, v, text.clone(), Kind::NoFact, now),
                false,
            )
        }
    };
    if other {
        let t = format!(
            "{}: {text}; площадка уже на паузе по другой причине — эту паузу бот не меняет (решение 413)",
            v.0
        );
        Box::pin(
            core.inner
                .deps
                .log(Level::Warn, "core.control", Some(&v.0), &t, None),
        )
        .await;
    }
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

/// Что делает: четвёртое ревью (С-1) — проверка снятия паузы «баланс не читается» без сделки (круг auto_resume.rs):
/// по каждой бирже с отметкой NoFact — балансы счетов (account_balances); торговый и перекладываемые на торговый счета
/// прочитаны — пауза снимается (lift). Принимает: ядро. Ошибок нет: не прочитано — пауза остаётся.
pub async fn recheck(core: &Core) {
    let marked: Vec<VenueId> = lk(&core.inner.risk)
        .venue_auto
        .iter()
        .filter(|(_, m)| m.kind == Kind::NoFact)
        .map(|(v, _)| v.clone())
        .collect();
    for v in marked {
        let Ok(gw) = core.inner.deps.gw.get(&v).await else {
            continue;
        };
        let ok = match gw.account_balances().await {
            Ok(accs) => accs.iter().all(|a| {
                a.error.is_none()
                    || gw.account_role(&a.account) == crate::gateway::contract::AccountRole::Other
            }),
            Err(_) => false,
        };
        if ok {
            Box::pin(lift(core, &v)).await;
        }
    }
}
