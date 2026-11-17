//! Файл: venues/bsc_dex/quote/shallow_build.rs
//! Что делает: решение 403 (задача t188 и её ревью, Н1) — построитель отбора мелких пулов пар вне пути котировки.
//! Одна задача на живой реестр:
//! - просыпается от записи в реестр (registry.rs put / update / retain → Memo::wrote, отметка пары в
//!   shallow_dirty.rs; при незаданном пределе записи не отмечаются и не будят) и от любой смены настроек;
//! - проход (в потоке блокирующих задач): предел dex.shallow_pools_per_pair, USDT и поколение настроек читаются ОДИН
//!   раз на проход. Полный проход (первый, смена настроек / USDT / предела, удаление пулов) — один снимок `Arc`
//!   записей реестра; иначе — только отмеченные пары и пары, чей встречный токен — монета отмеченной пары
//!   «встречный — USDT» (их пулы — индексом пары, без снимка всего реестра);
//! - у пары — отпечаток входов (shallow_stamps.rs Stamp); тот же — пара не пересчитывается; иначе — shallow_pick.rs calc
//!   (логика решения 393, гистерезис по прошлому отбору пары); пара ушла из реестра — её слот и память долой;
//! - отпечатки пересчитанных пар — в shallow_stamps.rs (пути вне котировки); снимок допуска пути котировки
//!   публикуется ТОЛЬКО если сменился допуск (хоть один пул вошёл или вышел) или предел; пулы, чей допуск сменился,
//!   отмечаются в реестре (registry.rs touch: версия и отметка фоновому графу).
//!
//! Сбой прохода (паника) — запись в tracing, следующий проход полный с пустой памятью гистерезиса; задача выходит,
//! когда хранилище настроек закрыто (система останавливается).
//! Кто вызывает: parts.rs (pools — ensure при подписке DEX на рынок); тесты (pass).
//! От чего зависит: quote/shallow_memo.rs, quote/shallow_stamps.rs, quote/shallow_pick.rs (limit, calc),
//! pools/registry.rs (snapshot, pair_arcs, pair_stamp, touch), tokens.rs, ctx.rs, tokio.
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

/// Память построителя между проходами: ключ прохода (поколение настроек, USDT, знаки, предел) и слоты пар (отпечаток и
/// отбор — он же прошлый отбор для гистерезиса).
#[derive(Default)]
pub struct State {
    key: Option<(u64, Address, u8, usize)>,
    slots: HashMap<Pair, Arc<Slot>>,
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

/// Один проход (см. шапку). Принимает: окружение, реестр, токены, память построителя. Ошибок нет.
pub fn pass(ctx: &Ctx, reg: &Registry, tokens: &Tokens, st: &mut State) {
    let memo = reg.shallow_memo();
    let n = shallow_pick::limit(ctx);
    memo.set_on(n > 0);
    let (dirty, full_mark) = memo.take_dirty();
    let (Some((usdt, dec)), true) = (usdt_dec(ctx, tokens), n > 0) else {
        // Предел не задан или USDT / его знаки не известны — мелкие не допускаются (как calc без USDT: кандидатов нет).
        st.key = None;
        st.slots.clear();
        memo.stamps().clear();
        let was = memo.load().visible().into_iter().collect();
        publish(reg, st, n, was);
        return;
    };
    let key = (ctx.settings.generation(), usdt, dec, n);
    let full = full_mark || st.key != Some(key);
    st.key = Some(key);
    let todo = if full {
        all_pairs(reg, st)
    } else {
        dirty_pairs(reg, st, dirty, usdt)
    };
    let mut rates: HashMap<Address, Vec<(u64, bool)>> = HashMap::new();
    let mut changed: Vec<(Pair, Option<Arc<Slot>>)> = Vec::new();
    let mut flipped: Vec<PoolRef> = Vec::new();
    for (pair, list) in todo {
        let old = st.slots.get(&pair).cloned();
        if list.is_empty() {
            if let Some(o) = old {
                flipped.extend(o.sel.iter().cloned());
                st.slots.remove(&pair);
                changed.push((pair, None));
            }
            continue;
        }
        let rate = if pair.1 == usdt {
            Vec::new()
        } else {
            rates
                .entry(pair.1)
                .or_insert_with(|| reg.pair_stamp(pair.1, usdt))
                .clone()
        };
        let mut pools: Vec<(u64, bool)> = list.iter().map(|e| (e.state_gen, e.asleep)).collect();
        pools.sort_unstable();
        let stamp = Stamp {
            settings: key.0,
            usdt,
            dec,
            pools,
            rate,
        };
        if old.as_ref().is_some_and(|o| o.stamp == stamp) {
            continue;
        }
        let prev = old.map(|o| Arc::clone(&o.sel)).unwrap_or_default();
        let sel = Arc::new(shallow_pick::calc(
            ctx, reg, tokens, dec, pair, n, &prev, &list,
        ));
        flipped.extend(prev.iter().filter(|r| !sel.contains(r)).cloned());
        flipped.extend(sel.iter().filter(|r| !prev.contains(r)).cloned());
        let slot = Arc::new(Slot { stamp, sel });
        st.slots.insert(pair, Arc::clone(&slot));
        changed.push((pair, Some(slot)));
    }
    memo.stamps().apply(changed);
    publish(reg, st, n, flipped);
}

/// Полный проход: все пары реестра (один снимок `Arc` записей) и пары памяти, которых в реестре больше нет (пустой
/// список — слот долой). Ошибок нет.
fn all_pairs(reg: &Registry, st: &State) -> Vec<(Pair, Vec<Arc<Entry>>)> {
    let mut g: HashMap<Pair, Vec<Arc<Entry>>> = HashMap::new();
    for e in reg.snapshot() {
        g.entry((e.coin, e.usdt)).or_default().push(e);
    }
    for p in st.slots.keys() {
        g.entry(*p).or_default();
    }
    g.into_iter().collect()
}

/// Проход по отметкам: отмеченные пары и пары памяти, чей встречный токен — монета отмеченной пары «X — USDT» (у них
/// сменился курс суммы пробы). Пулы — индексом пары (без снимка реестра). Ошибок нет.
fn dirty_pairs(
    reg: &Registry,
    st: &State,
    mut set: HashSet<Pair>,
    usdt: Address,
) -> Vec<(Pair, Vec<Arc<Entry>>)> {
    let quotes: HashSet<Address> = set.iter().filter(|p| p.1 == usdt).map(|p| p.0).collect();
    if !quotes.is_empty() {
        set.extend(st.slots.keys().filter(|p| quotes.contains(&p.1)).copied());
    }
    set.into_iter()
        .map(|p| (p, reg.pair_arcs(p.0, p.1)))
        .collect()
}

/// Публикация снимка допуска — только если допуск (`flipped` — вошедшие и вышедшие пулы) или предел сменились; затем
/// отметка в реестре сменивших допуск пулов (registry.rs touch). Принимает: реестр, память, предел, сменившие допуск.
/// Ошибок нет.
fn publish(reg: &Registry, st: &State, n: usize, flipped: Vec<PoolRef>) {
    let memo = reg.shallow_memo();
    let old = memo.load();
    if flipped.is_empty() && old.limit == n {
        return;
    }
    let sel = if n == 0 {
        HashMap::new()
    } else {
        st.slots
            .iter()
            .filter(|(_, s)| !s.sel.is_empty())
            .map(|(p, s)| (*p, Arc::clone(&s.sel)))
            .collect()
    };
    let new = Snap { limit: n, sel };
    let mut touch: HashSet<PoolRef> = flipped.into_iter().collect();
    if (old.limit == 0) != (n == 0) {
        touch.extend(old.visible());
        touch.extend(new.visible());
    }
    memo.publish(new);
    if !touch.is_empty() {
        reg.touch(&touch.into_iter().collect::<Vec<_>>());
    }
}

/// Адрес USDT и его знаки (из памяти, к узлу не ходит). None — не известны. Ошибок нет.
fn usdt_dec(ctx: &Ctx, tokens: &Tokens) -> Option<(Address, u8)> {
    let u = tokens.usdt(ctx).ok()?;
    Some((u, tokens.decimals_known(u)?))
}
