//! Файл: core/mid_wake.rs
//! Что делает: решение 349 — событие пула внутреннего участка маршрутов («промежуточная — USDT», «промежуточная —
//! промежуточная», например WBNB–USDT) ставит пересчёт монет, чьи маршруты идут через этот пул (кого — mid_wake_route.rs),
//! и сбрасывает их память котировок сигнала (решение 109). Раньше событие будило только монеты с адресом токена пула
//! (tick.rs, ticker_index by_tokens): монета X через WBNB ждала своего события или кадра стакана.
//! Ревью t160:
//! - Б2: поток тика только ставит отметку (note: замок, вставка, Notify — O(1)); пары пулов копятся, повтор пары в том же
//!   или более раннем блоке не будит второй раз; рассылку делает отдельная задача (run → pass), пары, накопленные к её
//!   ходу, сливаются в один проход (монета — один раз на проход). Поток тика не ждёт рассылку, канал ядра не растёт.
//! - Б3: разбуженная монета ставится как обычный тик её бирж (tick_sched::schedule_set — без отметки события пула и без
//!   coin_ticks): не занимает ограничитель signal.pool_event_min_interval_ms, не будит сон раньше
//!   signal.eval_min_interval_ms (как кадр стакана), удержание обычное (решение 317 skip_hold — только для события пула
//!   стороны DEX направления). Пересчёт помечается Trigger::MidPool (evaluate.rs take_marked) — отдельно на «Живом
//!   экране» и в core.signal_hold.
//! - Н3: coin_ticks (broadcast «Ожидания» застрявших сделок) пробуждение не шлёт.
//!
//! Под необязательной signal.mid_pool_wake (не задана или нет — 1 в 1 как до решения 349): безопасность по нагрузке не
//! доказана — разбуженные монеты пересчитываются наравне с остальными (приоритета ниже у пересчёта монет и у котировок
//! DEX нет: quote/cpu.rs — семафор без классов), число монет на событие — по замеру mid_wake_events / mid_wake_coins
//! (сводка dex.quote_count, log.quote_count_s).
//! Кто вызывает: core/tick.rs (peer, note), core/engine.rs (run), core/evaluate.rs (take_marked), core/quote_count_log.rs
//! (counts), тесты (pass).
//! От чего зависит: mid_wake_route.rs, intermediate.rs, ticker_index.rs, quote_cache.rs,
//! tick_sched.rs, settings.
//! Настройки: signal.mid_pool_wake; читает dex.intermediate_enabled, dex.intermediate_tokens, dex.router_mode,
//! dex.route_max_hops [площадка DEX].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use alloy_primitives::Address;
use tokio::sync::Notify;

use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::engine::{Core, lk, rd, wr};
pub use super::mid_wake_route::{reach, woken_tokens};

/// Пул события: площадка и токены.
type PairKey = (VenueId, Address, Address);

/// Соседи токенов по площадке.
type Peers = HashMap<(VenueId, Address), HashSet<Address>>;

/// Состояние пробуждений: ждущие пары (с наибольшим блоком), обработанные пары, монеты, помеченные к пересчёту.
#[derive(Default)]
pub struct State {
    /// Соседи токена по пулам площадки: (площадка, токен) → токены, с которыми у него есть пул. Растёт при первом пуле
    /// пары (tick.rs → peer); снятые пулы соседей не убирают — лишний сосед даёт лишний пересчёт, не потерю. Свой замок
    /// (не замок рынка): рассылка не держит рынок, поток тика не ждёт её (ревью Б2).
    peers: RwLock<Peers>,
    pending: Mutex<HashMap<PairKey, u64>>,
    done: Mutex<HashMap<PairKey, u64>>,
    marked: Mutex<HashSet<Asset>>,
    kick: Notify,
}

static EVENTS: AtomicU64 = AtomicU64::new(0);
static WOKEN: AtomicU64 = AtomicU64::new(0);

/// Счётчики с запуска: (проходов с пробуждением, разбуженных монет). Ошибок нет.
pub fn counts() -> (u64, u64) {
    (
        EVENTS.load(Ordering::Relaxed),
        WOKEN.load(Ordering::Relaxed),
    )
}

/// Что делает: отметка события пула из потока тика — O(1): пара в ожидание (наибольший блок), толчок задаче.
/// Принимает: ядро, площадку, токены пула, блок состояния. Ошибок нет: signal.mid_pool_wake не задана — ничего.
pub fn note(core: &Core, venue: &VenueId, tokens: (Address, Address), block: u64) {
    let s = &core.inner.deps.settings;
    if !s
        .flag("signal.mid_pool_wake", At::default())
        .unwrap_or(false)
    {
        return;
    }
    let key = (venue.clone(), tokens.0, tokens.1);
    let st = &core.inner.mid_wake;
    let mut p = lk(&st.pending);
    let b = p.entry(key).or_insert(block);
    *b = (*b).max(block);
    drop(p);
    st.kick.notify_one();
}

/// Что делает: первый пул пары площадки — соседи токенов (поток тика, O(1)). Принимает: ядро, площадку, токены пула.
/// Ошибок нет.
pub fn peer(core: &Core, venue: &VenueId, tokens: (Address, Address)) {
    let mut p = wr(&core.inner.mid_wake.peers);
    p.entry((venue.clone(), tokens.0))
        .or_default()
        .insert(tokens.1);
    p.entry((venue.clone(), tokens.1))
        .or_default()
        .insert(tokens.0);
}

/// Задача рассылки: на толчок — проход по накопленным парам. Работает, пока жив процесс. Ошибок нет.
pub async fn run(core: Core) {
    loop {
        core.inner.mid_wake.kick.notified().await;
        pass(&core);
    }
}

/// Что делает: один проход: ждущие пары (повтор пары не позже обработанного блока — пропуск) → по площадке объединение
/// разбуженных монет → каждой монете сброс памяти котировок, отметка MidPool и пересчёт по её биржам.
/// Принимает: ядро. Возвращает: сколько монет поставлено. Ошибок нет.
pub fn pass(core: &Core) -> usize {
    let st = &core.inner.mid_wake;
    let taken: Vec<(PairKey, u64)> = lk(&st.pending).drain().collect();
    let mut by_venue: HashMap<VenueId, Vec<(Address, Address)>> = HashMap::new();
    {
        let mut done = lk(&st.done);
        for (k, block) in taken {
            if done.get(&k).is_some_and(|b| *b >= block) {
                continue;
            }
            done.insert(k.clone(), block);
            by_venue.entry(k.0).or_default().push((k.1, k.2));
        }
    }
    let mut total = 0;
    for (venue, pools) in by_venue {
        total += wake_venue(core, &venue, &pools);
    }
    total
}

/// Что делает: монеты площадки по парам прохода (объединение) — сброс памяти, отметка, пересчёт. Принимает: ядро,
/// площадку, пары. Возвращает: сколько монет поставлено. Ошибок нет.
fn wake_venue(core: &Core, venue: &VenueId, pools: &[(Address, Address)]) -> usize {
    let s = &core.inner.deps.settings;
    let super::intermediate::Intermediate::On(mids) = super::intermediate::read(s, venue) else {
        return 0;
    };
    let Ok(usdt) = rd(&core.inner.usdt).clone() else {
        return 0;
    };
    let depth = extra_hops(core, venue, mids.len());
    let tokens: HashSet<Address> = {
        let map = rd(&core.inner.mid_wake.peers);
        let peers = |t: Address| -> Vec<Address> {
            map.get(&(venue.clone(), t))
                .map(|p| p.iter().copied().collect())
                .unwrap_or_default()
        };
        pools
            .iter()
            .flat_map(|p| woken_tokens(&peers, *p, &mids, depth, usdt))
            .collect()
    };
    if tokens.is_empty() {
        return 0;
    }
    let coins: HashSet<Asset> = {
        let tickers = rd(&core.inner.tickers);
        tokens
            .into_iter()
            .flat_map(|p| tickers.by_tokens((p, p)))
            .collect()
    };
    EVENTS.fetch_add(1, Ordering::Relaxed);
    WOKEN.fetch_add(coins.len() as u64, Ordering::Relaxed);
    let n = coins.len();
    for a in coins {
        if let Some(token) = a.bsc_contract {
            lk(&core.inner.quote_cache).bump(venue, token);
        }
        let venues = cex_venues(core, &a);
        if venues.is_empty() {
            continue;
        }
        lk(&core.inner.mid_wake.marked).insert(a.clone());
        super::tick_sched::schedule_set(core, a, venues);
    }
    n
}

/// Биржи монеты по справочнику (её направления с DEX и между биржами пересчитываются). Ошибок нет.
fn cex_venues(core: &Core, a: &Asset) -> HashSet<VenueId> {
    let coins = rd(&core.inner.coins);
    let Some(e) = super::ticker_index::find(core, &coins, a) else {
        return HashSet::new();
    };
    let kinds = rd(&core.inner.kinds);
    e.venues()
        .into_iter()
        .filter(|v| {
            kinds
                .iter()
                .any(|(k, kind)| k == v && *kind == VenueKind::Cex)
        })
        .collect()
}

/// Что делает: снимает отметку «разбужена промежуточным пулом» с монеты (evaluate.rs — один раз на пересчёт).
/// Принимает: ядро, монету. Возвращает: была ли отметка. Ошибок нет.
pub fn take_marked(core: &Core, a: &Asset) -> bool {
    lk(&core.inner.mid_wake.marked).remove(a)
}

/// Что делает: шаги «промежуточная — промежуточная» на пути через пул (H − 2). Принимает: ядро, площадку, число
/// промежуточных. Возвращает: шаги; режим graph без dex.route_max_hops — число промежуточных (все); классика — 0.
/// Ошибок нет.
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
