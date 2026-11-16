//! Файл: core/deposit_warm_pass.rs
//! Что делает: решение 316 — проход прогрева адресов депозита одной биржи: по очереди, с паузой
//! venue.deposit_warm_gap_ms между запросами, тот же метод шлюза deposit_address с той же монетой и сетью, что у
//! предпроверки (precheck_deposit.rs::target). Доработка по ревью:
//! - прогрев уступает сделке: перед проходом и перед КАЖДЫМ запросом — снимок занятости (deposit_warm_busy.rs); биржа
//!   занята — запросов нет, проход по ней останавливается и ждёт повтора (VenueState::wait = её пауза; повтор —
//!   deposit_warm.rs::wake);
//! - запрос идёт под запретом создания адреса (gateway/addr_claim.rs::forbid), если venue.deposit_warm_create не «да»;
//! - журнал core.deposit_warm: по бирже за проход — сколько запрошено, получено, отказов, первая причина кодом ошибки и
//!   короткой причиной (deposit_warm_plan.rs::reason) — без адресов и без значений memo/tag из текста адаптера.
//!
//! Кто вызывает: core/deposit_warm.rs (run).
//! От чего зависит: engine.rs, precheck_deposit.rs (target), deposit_warm_plan.rs, deposit_warm_busy.rs,
//! deposit_warm.rs (on), шлюз (deposit_address, addr_claim).
//! Настройки: venue.deposit_warm_gap_ms, venue.deposit_warm_create (по площадке; docs/SETTINGS.md).

use std::time::Duration;

use serde_json::json;

use crate::core::types::{Direction, VenueId};
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deposit_warm_busy::snapshot;
use super::deposit_warm_plan::{After, Memo, Tally, pick, reason};
use super::engine::Core;

const GAP: &str = "venue.deposit_warm_gap_ms";
const CREATE: &str = "venue.deposit_warm_create";

/// Причина остановки прохода, когда биржа занята сделкой.
const YIELDED: &str = "биржа занята сделкой — продолжение, когда освободится";

/// Память прогрева биржи: ответы по монетам, последняя записанная в журнал причина «не задано» и пауза повтора, если
/// проход уступил сделке (None — не ждёт).
#[derive(Default)]
pub struct VenueState {
    pub memo: Memo,
    reported: Option<String>,
    pub wait: Option<Duration>,
}

/// Что делает: проход прогрева одной биржи (см. шапку). Принимает: ядро, биржу, направления справочника, её память.
/// Возвращает: обновлённую память. Ошибок нет: пауза не задана — один раз журнал core.not_configured и прохода нет;
/// итог — журнал core.deposit_warm.
pub async fn venue_pass(
    core: &Core,
    v: &VenueId,
    dirs: &[Direction],
    mut s: VenueState,
) -> VenueState {
    let deps = &core.inner.deps;
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    s.wait = None;
    let gap = match deps.settings.integer(GAP, at) {
        Ok(ms) => Duration::from_millis(ms),
        Err(e) => {
            if s.reported.as_deref() != Some(e.message.as_str()) {
                let msg = format!("Прогрев адресов депозита {} не идёт: {}", v.0, e.message);
                deps.log(Level::Warn, "core.not_configured", Some(&v.0), msg, None)
                    .await;
                s.reported = Some(e.message);
            }
            return s;
        }
    };
    s.reported = None;
    let busy = snapshot(core);
    if busy.has(v) {
        s.wait = Some(gap);
        return s;
    }
    let list = pick(dirs.iter().cloned(), v, &busy);
    let gw = match deps.gw.get(v).await {
        Ok(g) => g,
        Err(_) => return s,
    };
    let mut t = Tally::default();
    for d in list {
        // Сети BSC у монеты нет — предпроверка откажет сама, запроса к бирже нет.
        let Ok(tg) = super::precheck_deposit::target(core, &d) else {
            continue;
        };
        let key = (tg.on.ticker.clone(), tg.net.clone());
        if !s.memo.todo(&key) {
            continue;
        }
        if t.asked > 0 {
            tokio::time::sleep(gap).await;
        }
        // Выключили посреди прохода — стоп.
        if !super::deposit_warm::on(core, v) {
            t.stopped = Some("прогрев выключен".into());
            break;
        }
        // Прогрев уступает сделке: проверка перед каждым запросом.
        if snapshot(core).has(v) {
            t.stopped = Some(YIELDED.into());
            s.wait = Some(gap);
            break;
        }
        t.asked += 1;
        let create = deps.settings.flag(CREATE, at).unwrap_or(false);
        let ask = gw.deposit_address(&tg.on, &tg.net);
        let res = if create {
            ask.await
        } else {
            crate::gateway::addr_claim::forbid(ask).await
        };
        let after = match &res {
            Ok(_) => {
                t.got += 1;
                s.memo.record(key, Ok(()))
            }
            Err(e) => {
                t.failed += 1;
                if t.first_err.is_none() {
                    let why = reason(&e.code);
                    t.first_err = Some(format!("{}: {:?} — {why}", d.asset.ticker, e.code));
                }
                s.memo.record(key, Err(&e.code))
            }
        };
        if after == After::Stop {
            if let Err(e) = &res {
                t.stopped = Some(format!("{:?}", e.code));
                t.locked = e.code == ErrorCode::SecretsLocked;
            }
            break;
        }
    }
    report(core, v, &t, &s.memo).await;
    s
}

/// Что делает: журнал core.deposit_warm за проход (кроме прохода без запросов и упора в закрытые секреты — Tally::silent).
/// Без адресов и значений из ответа биржи. Ошибок нет.
async fn report(core: &Core, v: &VenueId, t: &Tally, memo: &Memo) {
    if t.silent() {
        return;
    }
    let (ready, refused) = memo.counts();
    let level = if t.failed > 0 {
        Level::Warn
    } else {
        Level::Info
    };
    let msg = t.text(&v.0, ready, refused);
    let data = json!({
        "asked": t.asked, "got": t.got, "failed": t.failed,
        "ready_total": ready, "refused_total": refused,
        "first_error": t.first_err, "stopped": t.stopped,
    });
    core.inner
        .deps
        .log(level, "core.deposit_warm", Some(&v.0), msg, Some(data))
        .await;
}
