//! Файл: gateway/core_out.rs
//! Что делает: решение 324 — ОДИН общий набор «ждут доставки» (gateway/keep_last.rs) и ОДИН доставщик на канал событий
//! ядра — для пулов DEX и стаканов всех бирж (несколько наборов со своими доставщиками снова конкурировали бы за место
//! в канале: потери переезжали бы с одних площадок на другие). Порядок — общая очередь ключей (FIFO): ни пулы, ни
//! стаканы не голодают.
//! Где живёт набор: канал ядра создаёт ядро (core/engine.rs, market_tx) и раздаёт адаптерам через контракт
//! (Venue::subscribe_market(…, out: Sender<MarketEvent>)). Ни ядро, ни контракт не меняются: набор находится по самому
//! каналу — `of(&out)` ищет в реестре процесса запись с тем же каналом (Sender::same_channel; реестр держит только
//! WeakSender — канал не держится открытым) и создаёт её при первом обращении. Все клоны одного Sender получают один
//! и тот же набор; мёртвые записи (все Sender сняты или канал закрыт) выбрасываются при следующем обращении.
//! Доставщик — задача tokio, запускается при первом откладывании (Offer::Deferred { start: true }), держит Sender
//! только пока набор не пуст. Набор опустел — вызываются крючки «опустел» классов (запись журнала, on_drain).
//! Ревью 324 (Б2): состояние класса вне замка набора (настройки в памяти, счёт потерь, часы журнала) — Feed::side,
//! одно на класс (gateway/core_out_side.rs).
//! Кто вызывает: venues/bsc_dex/pools/emit_pool.rs (пулы), gateway/core_out_book.rs (стаканы и Invalidated бирж),
//! venues/bsc_dex/pools/lost_events.rs и gateway/book_lost_log.rs (счёт).
//! От чего зависит: gateway/keep_last.rs, gateway/core_out_item.rs, gateway/core_out_side.rs, gateway/events.rs, tokio.
//! Настроек нет (откладывать ли — решает вызывающий: dex.pool_events_keep, venue.book_events_keep).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use tokio::sync::mpsc::{Sender, WeakSender};

use super::core_out_item::{Class, Item, Key};
use super::core_out_side::Side;
use super::events::MarketEvent;
use super::keep_last::{Counts, Offer, Pending};

/// Крючок «набор опустел» класса (получает сам набор — без кольцевой ссылки). Ошибок нет.
pub type Hook = Arc<dyn Fn(&Feed) + Send + Sync>;

/// Общий набор канала ядра. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Feed {
    set: Pending<Key, Item>,
    hooks: Arc<Mutex<HashMap<Class, Hook>>>,
    /// Состояние класса вне замка набора (ревью 324, Б2).
    sides: Arc<Mutex<HashMap<Class, Arc<Side>>>>,
}

/// Реестр наборов процесса: канал (слабая ссылка) → набор.
type Reg = Mutex<Vec<(WeakSender<MarketEvent>, Feed)>>;

/// Реестр наборов (один на процесс). Ошибок нет.
fn reg() -> &'static Reg {
    static REG: OnceLock<Reg> = OnceLock::new();
    REG.get_or_init(Reg::default)
}

/// Замок без паники на отравлении. Ошибок нет.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Набор канала ядра (см. шапку). Принимает: канал. Возвращает: общий набор этого канала. Ошибок нет.
pub fn of(out: &Sender<MarketEvent>) -> Feed {
    let mut r = lock(reg());
    r.retain(|(w, _)| w.upgrade().is_some_and(|s| !s.is_closed()));
    if let Some((_, f)) = r
        .iter()
        .find(|(w, _)| w.upgrade().is_some_and(|s| s.same_channel(out)))
    {
        return f.clone();
    }
    let f = Feed::default();
    r.push((out.downgrade(), f.clone()));
    f
}

impl Feed {
    /// Предлагает событие (gateway/keep_last.rs offer); отложено первым — запускает доставщика. Принимает: канал,
    /// ключ, слот, «откладывать ли». Возвращает: Offer. Ошибок нет; нет рантайма tokio при запуске доставщика —
    /// паника tokio::spawn (адаптеры работают внутри рантайма).
    pub fn offer(
        &self,
        out: &Sender<MarketEvent>,
        key: Key,
        item: Item,
        keep: impl FnOnce() -> bool,
    ) -> Offer {
        let r = self.set.offer(out, key, item, keep);
        if r == (Offer::Deferred { start: true }) {
            self.spawn(out.clone());
        }
        r
    }

    /// Крючок «набор опустел» класса (заменяет прежний крючок этого класса). Ошибок нет.
    pub fn on_drain(&self, class: Class, hook: Hook) {
        lock(&self.hooks).insert(class, hook);
    }

    /// Забирает счёт класса с прошлой записи (keep_last.rs take). Ошибок нет.
    pub fn take(&self, class: &Class) -> Counts {
        self.set.take(class)
    }

    /// Счёт класса без обнуления (keep_last.rs peek). Ошибок нет.
    pub fn peek(&self, class: &Class) -> Counts {
        self.set.peek(class)
    }

    /// Сколько ключей ждёт (все классы). Ошибок нет.
    pub fn waiting(&self) -> usize {
        self.set.waiting()
    }

    /// Состояние класса вне замка набора (создаётся при первом обращении — make). Принимает: класс, создание.
    /// Возвращает: (состояние, создано ли сейчас — тогда вызывающий ставит крючок on_drain). Ошибок нет.
    pub fn side(&self, class: &Class, make: impl FnOnce() -> Side) -> (Arc<Side>, bool) {
        let mut m = lock(&self.sides);
        if let Some(s) = m.get(class) {
            return (s.clone(), false);
        }
        let s = Arc::new(make());
        m.insert(class.clone(), s.clone());
        (s, true)
    }

    /// Запускает доставщика; набор опустел — крючки классов (вне замков). Ошибок нет.
    fn spawn(&self, out: Sender<MarketEvent>) {
        let feed = self.clone();
        tokio::spawn(async move {
            if feed.set.clone().deliver(out).await {
                let hooks: Vec<Hook> = lock(&feed.hooks).values().cloned().collect();
                for h in hooks {
                    h(&feed);
                }
            }
        });
    }
}
