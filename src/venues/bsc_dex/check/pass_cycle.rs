//! Файл: venues/bsc_dex/check/pass_cycle.rs
//! Что делает: решение 401 (t186) — проба пулов менеджера через исполнитель кошельков. Ревью t186 (Н2): ОТДЕЛЬНАЯ задача
//! (pools/market.rs, класс Background очереди узла node/pace.rs — после сделок, проверок монет и крючков и котировок),
//! не внутри цикла крючков: обычные проверки крючков B/C пробы не ждут. Своего опроса нет:
//! - полный проход — раз в hooks.recheck_interval_s (та же настройка, что у цикла крючков);
//! - между проходами — очередь check/recheck.rs (pass): «Перепроверить» монеты, «новые пулы» и (ревью Н5) «налоги монеты
//!   проверены» (check/tax_cycle.rs) — пробуются её пулы, у которых итога ещё нет, он «не проверено» или метка отката.
//!
//! Порядок прохода (ревью Н2): сначала пулы с меткой отката сделки, затем пулы, которые котируются (не спят — pools/depth.rs:
//! спящий пул в котировку и сигналы не идёт, quote/select.rs), спящие — последними.
//! Пробуется пул шага менеджера (вид 19 / 20 / 21, Infinity Stable) с формой шага (entry.rs shape: ключ, валюты ERC-20 =
//! пара записи) — для КАЖДОГО кошелька с исполнителем v3 (версия «v3» у записи кошелька), если класс шага записью не
//! принят (решение 400: с крючком, Infinity Stable, Bin) или пул помечен откатом пробы сделки (pool_pass.rs on_sim).
//! Итог — факт pool_pass.rs; журнал dex.check — при смене итога (первый итог — «проходит» и «отвергнут», «не проверено» —
//! молча). Котировка читает факт напрямую (снимок pool_pass::kind) при каждом расчёте — отдельного сигнала не нужно.
//! Сбой узла, настройки или ключа — прежний факт остаётся; прежнего нет — «не проверено» с причиной.
//! Нагрузка: за полный проход — пулов-кандидатов × кошельков с v3 × 2 eth_simulateV1 подряд, по одной (оценка — заметка
//! t186 и docs/TASKS.md); пока ни у одного кошелька нет v3 — ни одной пробы.
//! Кто вызывает: pools/market.rs (запуск при подписке).
//! От чего зависит: check/{pass_probe, taxes_util, recheck}.rs, exec/executor/{entry, live_proof, pool_pass}.rs,
//! pools/{listen, exec_kind, families}.rs, settings_wait.rs, settings/accounts.
//! Настройки: hooks.recheck_interval_s (вся система); через pass_probe — check.probe_usdt, dex.quoter_max_diff_pct,
//! dex.deadline_s.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::Address;
use chrono::Utc;

use crate::core::types::PoolRef;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::exec::executor::pool_pass::{self, Fact, State};
use super::super::exec::executor::{entry, live_proof};
use super::super::pools::exec_kind;
use super::super::pools::families::Proto;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::super::settings_wait;
use super::pass_judge::Outcome;
use super::pass_probe;
use super::recheck::Queue;
use super::taxes_util::Sources;

/// Задача проб (см. шапку). Работает постоянно; не задан интервал — ждёт настройку (settings_wait.rs). Ошибок нет.
pub async fn run(p: Pools, queue: Queue) {
    loop {
        full(&p).await;
        if p.out.is_closed() {
            return;
        }
        let read = || {
            let s = p
                .ctx
                .settings
                .integer("hooks.recheck_interval_s", At::default());
            s.map(Duration::from_secs)
        };
        let Some(d) =
            settings_wait::until_ok(&p.ctx, "Проба пулов через исполнитель", "dex.check", read)
                .await
        else {
            return;
        };
        let until = tokio::time::Instant::now() + d;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(until) => break,
                _ = queue.notified() => {
                    let (coins, new_pools) = queue.take();
                    woken(&p, &coins, new_pools).await;
                    if p.out.is_closed() {
                        return;
                    }
                }
            }
        }
    }
}

/// Полный проход: все пулы-кандидаты в порядке шапки. Ошибок нет.
async fn full(p: &Pools) {
    let Some(xs) = executors(p) else { return };
    let all = p.reg.refs().into_iter().filter_map(|r| p.reg.entry(&r));
    each(p, ordered(&xs, all), false).await;
}

/// Внеочередной проход по очереди (ревью Н5): пулы переданных монет и при «новых пулах» — все, но только у кошельков,
/// где итога нет, он «не проверено» или метка отката сделки. Ошибок нет.
async fn woken(p: &Pools, coins: &HashSet<Address>, new_pools: bool) {
    let Some(xs) = executors(p) else { return };
    let all = p.reg.refs().into_iter().filter_map(|r| p.reg.entry(&r));
    let list = ordered(&xs, all.filter(|e| new_pools || coins.contains(&e.coin)));
    each(p, list, true).await;
}

/// Пробы по списку (запись реестра — свежая, на момент пробы). Принимает: состояние пулов, пулы, только неустоявшиеся.
/// Ошибок нет.
async fn each(p: &Pools, list: Vec<PoolRef>, unsettled: bool) {
    let sources = Sources::default();
    for r in list {
        let Some(e) = p.reg.get(&r).filter(|e| e.valid) else {
            continue;
        };
        pool(p, &sources, &e, unsettled).await;
        if p.out.is_closed() {
            return;
        }
    }
}

/// Исполнители v3 кошельков. None — ни у одного кошелька нет v3 (проб нет, реестр не обходится) или данные пулов
/// отстают (решение 72: вердикта нет). Ошибок нет.
fn executors(p: &Pools) -> Option<Vec<Address>> {
    let acc = p.ctx.settings.accounts();
    // Решение 432: исполнители этой площадки DEX (её сети).
    let xs: Vec<Address> = acc
        .wallets_at(p.ctx.place.id)
        .iter()
        .filter_map(pool_pass::v3)
        .collect();
    (!xs.is_empty() && !p.lag.busy()).then_some(xs)
}

/// Пулы-кандидаты (шаг менеджера, достоверные) по порядку: метка отката, не спят, спят. Принимает: исполнители v3,
/// записи реестра (без копий). Ошибок нет.
fn ordered(xs: &[Address], pools: impl Iterator<Item = Arc<Entry>>) -> Vec<PoolRef> {
    let mut list: Vec<(u8, PoolRef)> = pools
        .filter(|e| e.valid)
        .filter(|e| entry::shape(e).is_ok_and(|(k, _, _)| k.singleton()))
        .map(|e| {
            let rank = if marked(xs, &e.pool.reference) {
                0
            } else if e.asleep {
                2
            } else {
                1
            };
            (rank, e.pool.reference.clone())
        })
        .collect();
    list.sort_by_key(|(rank, _)| *rank);
    list.into_iter().map(|(_, r)| r).collect()
}

/// Есть ли у пула метка отката пробы сделки у какого-либо из исполнителей. Ошибок нет.
fn marked(xs: &[Address], r: &PoolRef) -> bool {
    xs.iter().any(|x| {
        pool_pass::get(*x, r).is_some_and(|f| matches!(f.state, State::Rejected { deal: true, .. }))
    })
}

/// Проба пула через исполнители всех кошельков с v3. Принимает: состояние пулов, источники встречного токена прохода,
/// пул, только ли неустоявшиеся итоги (нет итога, «не проверено», метка отката). Ошибок нет.
async fn pool(p: &Pools, sources: &Sources, e: &Entry, unsettled: bool) {
    let Ok((k, _, hooks)) = entry::shape(e) else {
        return;
    };
    if !k.singleton() {
        return;
    }
    let stable = e.family.proto == Proto::InfinityStable;
    let accepted = live_proof::accepted(k, stable, hooks);
    let r = &e.pool.reference;
    for w in p.ctx.settings.accounts().wallets_at(p.ctx.place.id) {
        let Some(x) = pool_pass::v3(&w) else { continue };
        let prev = pool_pass::get(x, r);
        let deal = matches!(
            prev.as_ref().map(|f| &f.state),
            Some(State::Rejected { deal: true, .. })
        );
        let settled = matches!(
            prev.as_ref().map(|f| &f.state),
            Some(State::Passed | State::Rejected { deal: false, .. })
        );
        if (accepted && !deal) || (unsettled && settled) {
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
