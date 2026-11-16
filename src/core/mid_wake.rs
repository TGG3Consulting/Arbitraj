//! Файл: core/mid_wake.rs
//! Что делает: решение 349 — событие пула промежуточной пары будит монеты, чьи маршруты идут через этот пул. Раньше
//! событие пула ставило пересчёт только монет с адресом одного из двух токенов пула (tick.rs, ticker_index by_tokens):
//! событие WBNB–USDT не будило монету X, торгуемую через WBNB, и не сбрасывало память котировок сигнала X (решение 109),
//! — сигнал X ждал своего события или кадра стакана.
//! Кого будим (по устройству маршрута адаптера DEX: внутренние узлы пути — только промежуточные монеты,
//! venues/bsc_dex/quote/route_paths.rs шапка; путь кончается в USDT): у пула события есть промежуточная монета m
//! (список dex.intermediate_tokens площадки, core/intermediate.rs). Достижимые промежуточные R — m и те, что связаны с
//! ними пулами «промежуточная — промежуточная» не дальше H − 2 шагов (H — dex.route_max_hops в режиме graph; не задан —
//! все промежуточные; классика — 2 хопа: R = промежуточные пула). Будятся все монеты справочника, у которых есть пул с
//! любой монетой из R (соседи по пулам площадки — market_state peers). Лишняя монета даёт лишний пересчёт, не потерю:
//! соседи не убираются при снятии пулов, а R — с запасом (без проверки направления пулов).
//! Каждой разбуженной монете — сброс памяти котировок сигнала (quote_cache bump) и пересчёт тем же путём, что событие
//! её пула (tick_sched::schedule_pool): события одного блока сливаются планом (tick_plan.rs: по монете не больше одного
//! идущего пересчёта и одного ждущего, сколько бы событий ни пришло), пауз по времени нет.
//! Включается необязательной signal.mid_pool_wake; не задана или нет — 1 в 1 как раньше (одно чтение настройки на событие
//! пула). Счётчики событий и разбуженных монет — для сводки (core/quote_count_log.rs).
//! Кто вызывает: core/tick.rs (событие пула).
//! От чего зависит: market_state.rs (peers), intermediate.rs, ticker_index.rs, quote_cache.rs, tick_sched.rs, settings.
//! Настройки: signal.mid_pool_wake; читает dex.intermediate_enabled, dex.intermediate_tokens, dex.router_mode,
//! dex.route_max_hops [площадка DEX].

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use alloy_primitives::Address;

use crate::core::types::{Asset, VenueId};
use crate::settings::read::At;

use super::engine::{Core, lk, rd};
use super::tick_plan::PoolMark;

/// События пула, по которым будились монеты (с промежуточной монетой в паре).
static EVENTS: AtomicU64 = AtomicU64::new(0);
/// Сколько монет поставлено в пересчёт этими событиями (с повторами).
static WOKEN: AtomicU64 = AtomicU64::new(0);

/// Счётчики с запуска: (событий, разбуженных монет). Ошибок нет.
pub fn counts() -> (u64, u64) {
    (
        EVENTS.load(Ordering::Relaxed),
        WOKEN.load(Ordering::Relaxed),
    )
}

/// Что делает: будит монеты, чьи маршруты идут через пул события (см. шапку). Принимает: ядро, площадку DEX, токены
/// пула, отметку события пула. Ошибок нет: настройка не задана, промежуточные выключены или USDT не известен — ничего.
pub fn wake(core: &Core, venue: &VenueId, tokens: (Address, Address), mark: PoolMark) {
    let s = &core.inner.deps.settings;
    if !s
        .flag("signal.mid_pool_wake", At::default())
        .unwrap_or(false)
    {
        return;
    }
    let super::intermediate::Intermediate::On(mids) = super::intermediate::read(s, venue) else {
        return;
    };
    let Ok(usdt) = rd(&core.inner.usdt).clone() else {
        return;
    };
    let start: Vec<Address> = [tokens.0, tokens.1]
        .into_iter()
        .filter(|t| mids.contains(t))
        .collect();
    if start.is_empty() {
        return;
    }
    let depth = extra_hops(core, venue, mids.len());
    // Соседи — под замком чтения рынка; монеты по адресам — после него (замки не вложены).
    let tokens_out: Vec<Address> = {
        let market = rd(&core.inner.market);
        let peers = |t: Address| -> Vec<Address> {
            market
                .peers
                .get(&(venue.clone(), t))
                .map(|p| p.iter().copied().collect())
                .unwrap_or_default()
        };
        let mut out = Vec::new();
        for r in reach(&peers, &start, &mids, depth) {
            out.extend(peers(r).into_iter().filter(|p| *p != usdt));
        }
        out
    };
    let coins: HashSet<Asset> = {
        let tickers = rd(&core.inner.tickers);
        tokens_out
            .into_iter()
            .flat_map(|p| tickers.by_tokens((p, p)))
            .collect()
    };
    EVENTS.fetch_add(1, Ordering::Relaxed);
    WOKEN.fetch_add(coins.len() as u64, Ordering::Relaxed);
    for a in coins {
        if let Some(token) = a.bsc_contract {
            // Решение 109: пулы маршрута монеты изменились — её котировки сигнала в памяти больше не годятся.
            lk(&core.inner.quote_cache).bump(venue, token);
        }
        super::tick_sched::schedule_pool(core, a, venue.clone(), mark);
    }
}

/// Что делает: сколько шагов «промежуточная — промежуточная» может быть на пути через пул (H − 2). Принимает: ядро,
/// площадку, число промежуточных. Возвращает: шаги; режим graph без dex.route_max_hops — число промежуточных (все);
/// классика — 0. Ошибок нет.
fn extra_hops(core: &Core, venue: &VenueId, mids: usize) -> usize {
    let s = &core.inner.deps.settings;
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    let graph = s.text("dex.router_mode", at).ok().as_deref() == Some("graph");
    if !graph {
        return 0;
    }
    match s.integer("dex.route_max_hops", at) {
        Ok(h) => (h as usize).saturating_sub(2),
        Err(_) => mids,
    }
}

/// Что делает: промежуточные монеты, достижимые от `start` по пулам «промежуточная — промежуточная» не больше чем за
/// `depth` шагов (поиск в ширину). Принимает: соседей токена, стартовые промежуточные, список промежуточных, шаги.
/// Возвращает: достижимые (со стартовыми). Ошибок нет.
pub fn reach(
    peers: &dyn Fn(Address) -> Vec<Address>,
    start: &[Address],
    mids: &[Address],
    depth: usize,
) -> Vec<Address> {
    let mut seen: Vec<Address> = start.to_vec();
    let mut front: Vec<Address> = start.to_vec();
    for _ in 0..depth {
        let mut next = Vec::new();
        for m in &front {
            for p in peers(*m) {
                if mids.contains(&p) && !seen.contains(&p) {
                    seen.push(p);
                    next.push(p);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        front = next;
    }
    seen
}
