//! Файл: core/sale_marks.rs
//! Что делает: решение 464 (t238, ревью) — отметки своих продаж для расчёта новых сделок (ride_settled.rs):
//! - ордер продажи в полёте по (площадка, монета) — от отправки ордера до его итога (credit_lock::selling: order →
//!   finish / Drop). Котировки-решения режима застрявшей монеты, перегона и «продать здесь» (ход sell_turn без ордера)
//!   сюда не попадают — они новых сделок не держат;
//! - число итогов продаж по (площадка, монета) (исполнено или исход неизвестен) — продажа, кончившаяся за время
//!   котировки новой сделки, видна по смене счётчика;
//! - сделки с неизвестным исходом продажи (Deal.sale_unsure, не закрытые и не «Беру на себя») по (площадка, монета) —
//!   набор ведётся при каждом сохранении сделки (ledger::save, ledger_load) и при итоге продажи в памяти
//!   (credit_lock::selling). Решение 480 (Тигран 21.11): монета НЕ блокируется — продажа новой сделки и «довезти» на
//!   этой площадке считают остаток такой сделки как ещё не исполненную продажу (худший случай, ride_settled::worst);
//!   человеку — уведомление deal.sale_unknown_block (одно на сделку, при входе в набор): сделку надо разобрать.
//!
//! Горячий путь: атомарные счётчики «ордеров в полёте» и «неясных» — 0: без замков; поиск по монете — по ссылке на
//! ключ (у монеты с адресом контракта без выделения памяти), площадки монеты — коротким списком.
//! Кто вызывает: credit_lock.rs (order, finish, track), ledger.rs и ledger_load.rs (track), ride_settled.rs (busy,
//! sales, unsure_ids), partial.rs и partial_back.rs (busy, done).
//! От чего зависит: coin_key.rs, deal.rs, engine.rs, core/types, deps.rs (уведомление).
//! Настройки: нет (базовое поведение; уведомление — событие deal.sale_unknown_block в настройках уведомлений).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Notify;

use crate::core::types::{Asset, VenueId};

use super::coin_key::CoinKey;
use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Событие уведомления: исход продажи сделки неизвестен — сделку надо разобрать (решение 480: монету не блокирует).
pub const EVENT: &str = "deal.sale_unknown_block";

/// Метка во внутренней причине каждого отказа из-за НАШЕЙ продажи (ride_settled.rs): по ней partial.rs отличает этот
/// отказ от прочих. В текстах для человека (blocked_text, Telegram) её нет.
pub const MARK: &str = "(решение 464: своя продажа)";

/// Отметки одной площадки монеты.
#[derive(Debug, Default)]
struct Mark {
    /// Ордеров продажи в полёте.
    orders: u32,
    /// Итогов продаж (исполнено или исход неизвестен).
    sales: u64,
}

/// Отметки продаж ядра (поле inner.sales).
#[derive(Debug, Default)]
pub struct Marks {
    by_coin: Mutex<HashMap<CoinKey, Vec<(VenueId, Mark)>>>,
    orders_live: AtomicUsize,
    /// ID сделки → (площадка, монета) с неизвестным исходом продажи.
    unsure: Mutex<HashMap<String, (VenueId, CoinKey)>>,
    unsure_live: AtomicUsize,
    /// Будит ждущих конца ордера продажи (partial.rs).
    pub done: Notify,
}

/// Ордер продажи в полёте; снимается finish или Drop.
pub struct Order<'a> {
    core: &'a Core,
    v: VenueId,
    key: CoinKey,
    /// Итог ордера отдан finish.
    finished: bool,
}

/// Правка отметки (площадка, монета) под замком. Ошибок нет.
fn edit(m: &Marks, v: &VenueId, key: &CoinKey, f: impl FnOnce(&mut Mark)) {
    let mut g = lk(&m.by_coin);
    let list = g.entry(key.clone()).or_default();
    match list.iter_mut().find(|(x, _)| x == v) {
        Some((_, mk)) => f(mk),
        None => {
            let mut mk = Mark::default();
            f(&mut mk);
            list.push((v.clone(), mk));
        }
    }
}

/// Что делает: отметить ордер продажи в полёте. Принимает: ядро, площадку, монету. Возвращает: отметку (снять —
/// finish или Drop). Ошибок нет.
pub fn order<'a>(core: &'a Core, v: &VenueId, a: &Asset) -> Order<'a> {
    let key = CoinKey::of(a);
    let m = &core.inner.sales;
    edit(m, v, &key, |x| x.orders += 1);
    m.orders_live.fetch_add(1, Ordering::AcqRel);
    Order {
        core,
        v: v.clone(),
        key,
        finished: false,
    }
}

impl Order<'_> {
    /// Что делает: итог ордера — `counted` (исполнено или исход неизвестен) прибавляет итог продажи ДО снятия
    /// отметки «в полёте». Принимает: признак. Ошибок нет.
    pub fn finish(mut self, counted: bool) {
        self.finished = true;
        if counted {
            edit(&self.core.inner.sales, &self.v, &self.key, |x| x.sales += 1);
        }
    }
}

/// ВНИМАНИЕ: отметка снимается и без finish — будущее продажи отменено (drop) или паника в задаче. Транзакция могла
/// уже уйти в сеть, итога ядро не знает: такой конец засчитывается как итог продажи (счётчик растёт) — расчёт, шедший
/// в это время, получит отказ «исполнилась, пока шла котировка», а не посчитает пул без неё.
impl Drop for Order<'_> {
    fn drop(&mut self) {
        let m = &self.core.inner.sales;
        let unknown = !self.finished;
        let dec = |x: &mut Mark| {
            x.orders = x.orders.saturating_sub(1);
            if unknown {
                x.sales += 1;
            }
        };
        edit(m, &self.v, &self.key, dec);
        m.orders_live.fetch_sub(1, Ordering::AcqRel);
        m.done.notify_waiters();
    }
}

/// Отметка (площадка, монета) без создания. Ошибок нет.
fn read<R>(core: &Core, v: &VenueId, a: &Asset, f: impl Fn(&Mark) -> R) -> Option<R> {
    let g = lk(&core.inner.sales.by_coin);
    let list = g.get(&CoinKey::of(a))?;
    list.iter().find(|(x, _)| x == v).map(|(_, mk)| f(mk))
}

/// Что делает: идёт ли сейчас ордер продажи монеты на площадке. Принимает: ядро, площадку, монету. Ошибок нет.
pub fn busy(core: &Core, v: &VenueId, a: &Asset) -> bool {
    if core.inner.sales.orders_live.load(Ordering::Acquire) == 0 {
        return false;
    }
    read(core, v, a, |m| m.orders > 0).unwrap_or(false)
}

/// Что делает: число итогов продаж монеты на площадке. Принимает: ядро, площадку, монету. Ошибок нет.
pub fn sales(core: &Core, v: &VenueId, a: &Asset) -> u64 {
    read(core, v, a, |m| m.sales).unwrap_or(0)
}

/// Что делает: внести или убрать сделку в наборе «исход продажи неизвестен» по её записи; вошла впервые —
/// уведомление человеку (отдельной задачей, сделку не держит). Принимает: ядро, сделку. Ошибок нет.
pub fn track(core: &Core, d: &Deal) {
    let m = &core.inner.sales;
    let on = d.sale_unsure && !matches!(d.step, Step::Closed | Step::TakenOver);
    if !on && m.unsure_live.load(Ordering::Acquire) == 0 {
        return;
    }
    let new = {
        let mut g = lk(&m.unsure);
        let new = if on {
            let at = (d.at.clone(), CoinKey::of(&d.direction.asset));
            g.insert(d.id.clone(), at).is_none()
        } else {
            g.remove(&d.id);
            false
        };
        m.unsure_live.store(g.len(), Ordering::Release);
        new
    };
    if new {
        let (core, text, v) = (core.clone(), blocked_text(d), d.at.0.clone());
        tokio::spawn(async move {
            core.inner.deps.notify(EVENT, Some(&v), &text).await;
        });
    }
}

/// Что делает: все сделки с неизвестным исходом продажи монеты на площадке. Принимает: ядро, площадку, монету.
/// Возвращает: ID по порядку (пусто — нет). Ошибок нет.
pub fn unsure_ids(core: &Core, v: &VenueId, a: &Asset) -> Vec<String> {
    let m = &core.inner.sales;
    if m.unsure_live.load(Ordering::Acquire) == 0 {
        return vec![];
    }
    let key = CoinKey::of(a);
    let mut ids: Vec<String> = lk(&m.unsure)
        .iter()
        .filter(|(_, (x, k))| x == v && k == &key)
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort_unstable();
    ids
}

/// Текст уведомления deal.sale_unknown_block для человека (Telegram). Ошибок нет.
pub fn blocked_text(d: &Deal) -> String {
    format!(
        "исход продажи сделки {} ({} {} на {}) неизвестен — разберите сделку (экран «Сделки» / «Ручное управление»). \
         Монета не заблокирована: новые сделки и «довезти» {} на {} не ждут, а считают эту продажу как ещё не \
         исполненную (худший случай)",
        d.id, d.qty, d.direction.asset.ticker, d.at.0, d.direction.asset.ticker, d.at.0
    )
}
