//! Файл: venues/bsc_dex/check/pass_cycle.rs
//! Что делает: решение 401 (t186) — проба пула менеджера через исполнитель в цикле крючков (check/cycle.rs one): тот же
//! проход и та же перепроверка (hooks.recheck_interval_s, «Перепроверить», «новые пулы», «пул готов»), без своего опроса.
//! Пробуется пул шага менеджера (вид 19 / 20 / 21, Infinity Stable), форма шага которого строится (entry.rs shape: ключ,
//! валюты ERC-20 = пара записи), — для КАЖДОГО кошелька с исполнителем v3 (сверен: версия «v3» у записи кошелька,
//! exec/executor/onchain.rs), если:
//! - класс шага записью не принят (решение 400: с крючком, Infinity Stable, Bin) — проба решает, исполним ли пул; или
//! - пул помечен «отвергнут» откатом пробы сделки (pool_pass.rs on_sim) — перепроверка снимает или подтверждает метку.
//!
//! Пулы принятых записью классов без метки не пробуются (нагрузка). Итог — факт pool_pass.rs (журнал dex.check — при
//! смене итога; первый итог — «проходит» и «отвергнут», «не проверено» — молча); поколение итогов проверок растёт
//! (эпоха котировки, решение 352).
//! Сбой узла, настройки или ключа — прежний факт остаётся; прежнего нет — «не проверено» с причиной.
//! Нагрузка: пулов-кандидатов × кошельков с v3 × 2 eth_simulateV1 за проход (через общую очередь узла node/pace.rs);
//! пока ни у одного кошелька нет v3 — ни одной пробы.
//! Кто вызывает: check/cycle.rs (one).
//! От чего зависит: check/{pass_probe, taxes_util}.rs, exec/executor/{entry, live_proof, pool_pass}.rs,
//! pools/{listen, exec_kind, families}.rs, settings/accounts.
//! Настройки: нет своих (через pass_probe — check.probe_usdt, dex.quoter_max_diff_pct, dex.deadline_s).

use chrono::Utc;

use crate::storage::journal::Level;

use super::super::exec::executor::pool_pass::{self, Fact, State};
use super::super::exec::executor::{entry, live_proof};
use super::super::pools::exec_kind;
use super::super::pools::families::Proto;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::pass_judge::Outcome;
use super::pass_probe;
use super::taxes_util::Sources;

/// Проба пула через исполнители всех кошельков с v3 (см. шапку). Принимает: состояние пулов, источники встречного
/// токена прохода, пул. Ошибок нет.
pub async fn pool(p: &Pools, sources: &Sources, e: &Entry) {
    let Ok((k, _, hooks)) = entry::shape(e) else {
        return;
    };
    if !k.singleton() {
        return;
    }
    let stable = e.family.proto == Proto::InfinityStable;
    let accepted = live_proof::admits(k, stable, hooks).is_ok();
    let r = &e.pool.reference;
    for w in p.ctx.settings.accounts().wallets() {
        let Some(x) = pool_pass::v3(&w) else { continue };
        let prev = pool_pass::get(x, r);
        let deal = matches!(
            prev.as_ref().map(|f| &f.state),
            Some(State::Rejected { deal: true, .. })
        );
        if accepted && !deal {
            continue;
        }
        let ctx = p.ctx.with_account(&w.id);
        if !exec_kind::via_executor(&ctx, e.family) {
            continue;
        }
        let state = match pass_probe::run(p, &ctx, sources, e, (x, k, hooks)).await {
            Ok((o, block)) => (state_of(o), block),
            // Сбой узла, настройки, ключа — прежний факт остаётся (в том числе метка отката сделки).
            Err(_) if prev.is_some() => continue,
            Err(err) => (State::Unchecked(err.message), None),
        };
        let fact = Fact {
            state: state.0,
            at: Utc::now(),
            block: state.1,
        };
        if !pool_pass::put(x, r, fact.clone()) {
            continue;
        }
        // Итог сменился — эпоха котировки (решение 352) и журнал.
        p.checks.update_pool_if(r, |_| true);
        let level = match (&prev, &fact.state) {
            (_, State::Rejected { .. }) => Some(Level::Warn),
            (None, State::Passed) | (Some(_), _) => Some(Level::Info),
            (None, State::Unchecked(_)) => None,
        };
        if let Some(level) = level {
            let line = pool_pass::line(&w.name, Some(&fact));
            let msg = format!("{} {}: {line}", e.family.title, e.ticker);
            p.ctx.log(level, "dex.check", msg, None).await;
        }
    }
}

/// Итог пробы → факт. Ошибок нет.
fn state_of(o: Outcome) -> State {
    match o {
        Outcome::Passed => State::Passed,
        Outcome::Rejected(why) => State::Rejected { why, deal: false },
        Outcome::Unchecked(why) => State::Unchecked(why),
    }
}
