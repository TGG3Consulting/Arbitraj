//! Файл: venues/bsc_dex/quote/shallow_build.rs
//! Что делает: решение 403 (задача t188) — построитель снимка отбора мелких пулов пар (quote/shallow_memo.rs) вне пути
//! котировки. Одна задача на живой реестр:
//! - просыпается от записи в реестр (registry.rs put / update / retain → Memo::poke; при незаданном пределе записи её
//!   не будят) и от любой смены настроек (подписка хранилища);
//! - проход (в потоке блокирующих задач): предел dex.shallow_pools_per_pair и USDT читаются ОДИН раз на проход; записи
//!   реестра — один снимок `Arc` (без копий); для каждой пары — отпечаток входов (поколение настроек, USDT и знаки,
//!   поколения и «спит» пулов пары и пулов «встречный — USDT»); отпечаток тот же — прежний отбор, иначе пересчёт
//!   (shallow_pick.rs calc — та же логика решения 393, гистерезис по прошлому отбору пары);
//! - публикует новый снимок, если что-то сменилось; пулы, чей допуск сменился, отмечает в реестре (registry.rs touch:
//!   версия и отметка фоновому графу) — граф пересобирается и с ними, как раньше при любой записи.
//!
//! Сбой прохода (паника) — запись в tracing, следующий проход с пустой памятью гистерезиса; задача выходит, когда
//! хранилище настроек закрыто (система останавливается).
//! Кто вызывает: parts.rs (pools — ensure при подписке DEX на рынок).
//! От чего зависит: quote/shallow_memo.rs, quote/shallow_pick.rs (limit, calc), pools/registry.rs (snapshot, touch),
//! tokens.rs, ctx.rs, tokio.
//! Настройки: dex.shallow_pools_per_pair, venue.quote_asset [bsc_dex] (через shallow_pick.rs и tokens.rs).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use alloy_primitives::Address;
use tokio::sync::broadcast::error::RecvError;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;
use super::shallow_memo::{Pair, Slot, Snap, Stamp};
use super::shallow_pick;

/// Память построителя между проходами: последний отбор каждой пары (гистерезис, как `last` решения 393).
#[derive(Default)]
pub struct State {
    last: HashMap<Pair, Arc<Vec<PoolRef>>>,
}

/// Запуск построителя у живого реестра (один раз; повторно — внеочередной проход). Вне среды tokio — не запускается
/// (пути вне котировки считают отбор сами, shallow_pick.rs pick). Принимает: окружение, реестр, токены. Ошибок нет.
pub fn ensure(ctx: &Ctx, reg: &Registry, tokens: &Tokens) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let memo = reg.shallow_memo();
    if !memo.start_once() {
        memo.wake_now();
        return;
    }
    rt.spawn(run(ctx.clone(), reg.clone(), tokens.clone()));
}

/// Цикл построителя (см. шапку). Принимает: окружение, реестр, токены. Ошибок нет.
async fn run(ctx: Ctx, reg: Registry, tokens: Tokens) {
    let memo = reg.shallow_memo().clone();
    let mut changes = ctx.settings.subscribe();
    let mut st = State::default();
    loop {
        // Подписка на пробуждение — до прохода: запись во время прохода не теряется.
        let wake = memo.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        let (c, r, t) = (ctx.clone(), reg.clone(), tokens.clone());
        let mut s = std::mem::take(&mut st);
        match tokio::task::spawn_blocking(move || {
            pass(&c, &r, &t, &mut s);
            s
        })
        .await
        {
            Ok(s) => st = s,
            Err(e) => tracing::error!("отбор мелких пулов (решение 403): проход прерван — {e}"),
        }
        tokio::select! {
            _ = &mut wake => {}
            r = changes.recv() => {
                if matches!(r, Err(RecvError::Closed)) {
                    return;
                }
            }
        }
    }
}

/// Один проход (см. шапку): отбор всех пар по текущему реестру и публикация. Принимает: окружение, реестр, токены,
/// память построителя. Ошибок нет.
pub fn pass(ctx: &Ctx, reg: &Registry, tokens: &Tokens, st: &mut State) {
    let memo = reg.shallow_memo();
    let old = memo.load();
    let n = shallow_pick::limit(ctx);
    memo.set_on(n > 0);
    let (Some((usdt, dec)), true) = (usdt_dec(ctx, tokens), n > 0) else {
        // Предел не задан или USDT / его знаки не известны — мелкие не допускаются (как calc без USDT: кандидатов нет).
        let snap = Snap {
            limit: n,
            slots: HashMap::new(),
        };
        finish(reg, &old, snap);
        return;
    };
    let settings = ctx.settings.generation();
    let mut groups: HashMap<Pair, Vec<Arc<Entry>>> = HashMap::new();
    for e in reg.snapshot() {
        groups.entry((e.coin, e.usdt)).or_default().push(e);
    }
    let marks = |p: &Pair| -> Vec<(u64, bool)> {
        let mut v: Vec<(u64, bool)> = groups
            .get(p)
            .map(|l| l.iter().map(|e| (e.state_gen, e.asleep)).collect())
            .unwrap_or_default();
        v.sort_unstable();
        v
    };
    let mut slots = HashMap::with_capacity(groups.len());
    let mut fresh = false;
    for (pair, list) in &groups {
        let stamp = Stamp {
            settings,
            usdt,
            dec,
            pools: marks(pair),
            rate: if pair.1 == usdt {
                Vec::new()
            } else {
                marks(&(pair.1, usdt))
            },
        };
        let sel = match old.slots.get(pair) {
            Some(s) if old.limit == n && s.stamp == stamp => Arc::clone(&s.sel),
            _ => {
                fresh = true;
                let prev = st.last.get(pair).cloned().unwrap_or_default();
                let v = Arc::new(shallow_pick::calc(
                    ctx, reg, tokens, dec, *pair, n, &prev, list,
                ));
                st.last.insert(*pair, Arc::clone(&v));
                v
            }
        };
        slots.insert(*pair, Slot { stamp, sel });
    }
    if fresh || slots.len() != old.slots.len() || old.limit != n {
        finish(reg, &old, Snap { limit: n, slots });
    }
}

/// Публикация снимка и отметка в реестре пулов, чей допуск сменился (registry.rs touch). Снимок тот же по допуску и
/// пределу — не публикуется. Принимает: реестр, прежний и новый снимки. Ошибок нет.
fn finish(reg: &Registry, old: &Snap, new: Snap) {
    let (a, b) = (admitted(old), admitted(&new));
    let flipped: Vec<PoolRef> = a.symmetric_difference(&b).cloned().collect();
    let same = old.limit == new.limit && old.slots.len() == new.slots.len() && flipped.is_empty();
    if same && new.slots.is_empty() {
        return;
    }
    reg.shallow_memo().publish(new);
    if !flipped.is_empty() {
        reg.touch(&flipped);
    }
}

/// Все допущенные пулы снимка. Ошибок нет.
fn admitted(s: &Snap) -> HashSet<PoolRef> {
    if s.limit == 0 {
        return HashSet::new();
    }
    s.slots
        .values()
        .flat_map(|x| x.sel.iter().cloned())
        .collect()
}

/// Адрес USDT и его знаки (из памяти, к узлу не ходит). None — не известны. Ошибок нет.
fn usdt_dec(ctx: &Ctx, tokens: &Tokens) -> Option<(Address, u8)> {
    let u = tokens.usdt(ctx).ok()?;
    Some((u, tokens.decimals_known(u)?))
}
