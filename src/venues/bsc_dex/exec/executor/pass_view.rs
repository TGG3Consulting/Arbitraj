//! Файл: venues/bsc_dex/exec/executor/pass_view.rs
//! Что делает: решение 401 (t186) — проба пулов менеджера через исполнитель кошелька на экранах:
//! - details: строки карточки пула (экран «Монеты», pools/depth_facts.rs → PoolDepth.details) — класс шага принят
//!   записью или нет и по каждому кошельку с исполнителем v3: проходит обе стороны / отвергнут (причина) / не проверено,
//!   с временем и блоком пробы;
//! - proofs: таблица классов установленного или готового к развёртыванию исполнителя (GET /api/wallets → executor_v3_classes) — к приёмке класса по
//!   записи (live_proof.rs) сводка пулов класса у ЭТОГО кошелька: прошли / отвергнуты / не проверено.
//!
//! Кто вызывает: pools/depth_facts.rs, venue.rs (dex_executor_classes).
//! От чего зависит: exec/executor/{entry, live_proof, live_class, pool_pass}.rs, pools/{registry, families}.rs, ctx.rs.
//! Настройки: нет.

use crate::core::types::ExecutorClassProof;

use super::super::super::ctx::Ctx;
use super::super::super::pools::families::Proto;
use super::super::super::pools::registry::{Entry, Registry};
use super::code::{self, Version};
use super::live_class::Class;
use super::pool_pass::{self, State};
use super::{entry, live_proof};

/// Версия исполнителя кошелька окружения (не сверена — v3, как pool_gate.rs) и сеть площадки (решение 461). Ошибок нет.
fn pair_of(ctx: &Ctx) -> (Version, Option<u64>) {
    let ver = ctx.exec_info().and_then(|(_, v)| v).unwrap_or(Version::V3);
    (ver, live_proof::chain_of(ctx))
}

/// Класс шага пула менеджера и принят ли он записью пары кошелька (решение 461). None — не шаг менеджера или форма
/// шага не строится. Ошибок нет.
fn class(ctx: &Ctx, e: &Entry) -> Option<(Class, bool)> {
    let (k, _, hooks) = entry::shape(e).ok()?;
    let stable = e.family.proto == Proto::InfinityStable;
    let c = live_proof::class_of(k, stable, hooks)?;
    let (ver, chain) = pair_of(ctx);
    Some((c, live_proof::accepted_for(ver, chain, k, stable, hooks)))
}

/// Строки карточки пула (см. шапку). Пусто — не шаг менеджера или ни у одного кошелька нет исполнителя v3.
/// Принимает: окружение, пул. Ошибок нет.
pub fn details(ctx: &Ctx, e: &Entry) -> Vec<String> {
    let Some((c, accepted)) = class(ctx, e) else {
        return Vec::new();
    };
    let wallets: Vec<_> = ctx
        .settings
        .accounts()
        .wallets_at(ctx.place.id)
        .into_iter()
        .filter_map(|w| pool_pass::v3(&w).map(|x| (w.name, x)))
        .collect();
    if wallets.is_empty() {
        return Vec::new();
    }
    let head = if accepted {
        format!(
            "исполнитель v3: класс «{}» принят записью (решение 400)",
            c.label()
        )
    } else {
        format!(
            "исполнитель v3: класс «{}» записью не принят — через исполнитель только после пробы этого пула (решение 401)",
            c.label()
        )
    };
    let mut out = vec![head];
    for (name, x) in wallets {
        let f = pool_pass::get(ctx.place.id, x, &e.pool.reference);
        out.push(pool_pass::line(&name, f.as_ref()));
    }
    out
}

/// Классы шага со сводкой по пулам кошелька окружения (см. шапку). Принимает: окружение (кошелёк), реестр пулов.
/// Ошибок нет.
pub fn proofs(ctx: &Ctx, reg: &Registry) -> Vec<ExecutorClassProof> {
    let wallet = ctx.wallet().ok();
    if wallet.as_ref().is_some_and(|w| w.executor.is_none()) {
        // На этой сети ещё нет адреса: показываем ту же пару, что проверяет кнопка развёртывания.
        let Ok(version) = code::current_version() else {
            return Vec::new();
        };
        let mut list = live_proof::proofs(version, live_proof::chain_of(ctx));
        for p in &mut list {
            p.live = format!(
                "исполнитель {} ещё не развёрнут в этой сети — пробы пулов кошелька отсутствуют",
                version.label()
            );
        }
        return list;
    }
    let (ver, chain) = pair_of(ctx);
    let mut list = live_proof::proofs(ver, chain);
    let Some(x) = wallet.as_ref().and_then(pool_pass::v3) else {
        for p in &mut list {
            p.live = "нет сверенного исполнителя для пробы пулов кошелька".into();
        }
        return list;
    };
    // По классу: (прошли, отвергнуты, не проверено).
    let mut counts: Vec<(Class, [usize; 3])> = Vec::new();
    for r in reg.refs() {
        let Some(e) = reg.get(&r) else { continue };
        let Some((c, _)) = class(ctx, &e) else {
            continue;
        };
        let slot = match pool_pass::get(ctx.place.id, x, &r).map(|f| f.state) {
            Some(State::Passed) => 0,
            Some(State::Rejected { .. }) => 1,
            Some(State::Unchecked(_)) | None => 2,
        };
        match counts.iter_mut().find(|(k, _)| *k == c) {
            Some((_, n)) => n[slot] += 1,
            None => {
                let mut n = [0; 3];
                n[slot] = 1;
                counts.push((c, n));
            }
        }
    }
    for p in &mut list {
        let n = counts
            .iter()
            .find(|(c, _)| c.label() == p.class)
            .map_or([0; 3], |(_, n)| *n);
        p.live = if p.accepted {
            format!(
                "класс принят записью — пулы исполнимы без пробы; отвергнуты пробой или откатом сделки: {}",
                n[1]
            )
        } else {
            format!(
                "пулы кошелька (проба через исполнитель, решение 401): прошли {}, отвергнуты {}, не проверено {}",
                n[0], n[1], n[2]
            )
        };
    }
    list
}
