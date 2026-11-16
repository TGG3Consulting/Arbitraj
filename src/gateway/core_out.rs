//! Файл: gateway/core_out.rs
//! Что делает: решение 324 — ОДИН общий набор «ждут доставки» (gateway/keep_last.rs) и ОДИН доставщик на канал событий
//! ядра — для пулов DEX и стаканов всех бирж (несколько наборов со своими доставщиками снова конкурировали бы за место
//! в канале: потери переезжали бы с одних площадок на другие). Порядок — общая очередь ключей (FIFO): ни пулы, ни
//! стаканы не голодают.
//! Где живёт набор: канал ядра создаёт ядро (core/engine.rs, market_tx) и раздаёт адаптерам через контракт
//! (Venue::subscribe_market(…, out: Sender<MarketEvent>)). Ни ядро, ни контракт не меняются: набор находится по самому
//! каналу — `of(&out)` (gateway/core_out_reg.rs) ищет в реестре процесса запись с тем же каналом
//! (Sender::same_channel; реестр держит только WeakSender — канал не держится открытым) и создаёт её при первом
//! обращении. Все клоны одного Sender получают один и тот же набор; мёртвые записи выбрасываются.
//! Доставщик — задача tokio, запускается при первом откладывании (Offer::Deferred { start: true }), держит Sender
//! только пока набор не пуст. Набор опустел — вызываются крючки «опустел» классов (запись журнала, on_drain).
//! Ревью 324 (Б2): состояние класса вне замка набора (настройки в памяти, счёт потерь, часы журнала) — Feed::side,
//! одно на класс (gateway/core_out_side.rs). Ревью 324 (Б1): быстрый путь Feed::fast (набор пуст — одна атомарная
//! загрузка и try_reserve, без замка, ключа и настройки), медленный Feed::slow; проверка «пул годен» (Live) — одна на
//! площадку DEX (Feed::live), доставщик зовёт её по ключу пула (keep_last Check), а не клон Live в каждом слоте.
//! Решение 333: здесь же живёт очередь «ждут места» сообщений, по которым ядро решает о деньгах, и уведомлений
//! (Feed::must, gateway/core_out_must.rs) — одна на канал, по самому каналу, как набор.
//! Кто вызывает: venues/bsc_dex/pools/emit_pool.rs (пулы), gateway/core_out_book.rs (стаканы и Invalidated бирж),
//! gateway/core_out_must.rs (Feed::must),
//! venues/bsc_dex/pools/lost_events.rs и gateway/book_lost_log.rs (счёт).
//! От чего зависит: gateway/keep_last.rs, gateway/core_out_item.rs, gateway/core_out_side.rs, gateway/events.rs, tokio.
//! Настроек нет (откладывать ли — решает вызывающий: dex.pool_events_keep, venue.book_events_keep).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use tokio::sync::mpsc::Sender;

use crate::core::types::VenueId;

use super::core_out_item::{Class, Item, Key, Live};
use super::core_out_side::Side;
use super::events::MarketEvent;
use super::keep_last::{Ask, Check, Counts, Fast, Offer, Pending};

/// Крючок «набор опустел» класса (получает сам набор — без кольцевой ссылки). Ошибок нет.
pub type Hook = Arc<dyn Fn(&Feed) + Send + Sync>;

/// Общий набор канала ядра. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Feed {
    set: Pending<Key, Item>,
    hooks: Arc<Mutex<HashMap<Class, Hook>>>,
    /// Состояние класса вне замка набора (ревью 324, Б2).
    sides: Arc<Mutex<HashMap<Class, Arc<Side>>>>,
    /// Проверка «пул годен» по площадке DEX (ревью 324, Б1).
    lives: Lives,
    /// Решение 333: очередь «ждут места» сообщений рода Money / Info этого канала (gateway/core_out_must.rs).
    must: Arc<super::core_out_must::Queue>,
}

/// Проверки «пул годен» по площадке.
type Lives = Arc<RwLock<HashMap<VenueId, Live>>>;

/// Проверка доставщика: пул DEX — по Live его площадки (нет Live — годен), стакан биржи — без проверки.
struct Pools(Lives);

impl Check<Key, MarketEvent> for Pools {
    fn needs(&self, key: &Key) -> bool {
        matches!(key, Key::Pool(..))
    }

    fn live(&self, key: &Key, msg: &MarketEvent) -> bool {
        let Key::Pool(v, _) = key else {
            return true;
        };
        // Live берётся из-под замка карты и вызывается вне его (Live читает реестр пулов).
        let f = self
            .0
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(v)
            .cloned();
        f.is_none_or(|f| f(msg))
    }
}

/// Замок без паники на отравлении. Ошибок нет.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub use super::core_out_reg::of;

impl Feed {
    /// Быстрый путь (keep_last.rs fast): набор пуст — без замка. Принимает: канал. Возвращает: Fast. Ошибок нет.
    pub fn fast<'a>(&self, out: &'a Sender<MarketEvent>) -> Fast<'a, MarketEvent> {
        self.set.fast(out)
    }

    /// Медленный путь (keep_last.rs slow); отложено первым — запускает доставщика. Принимает: канал, ключ, слот,
    /// «откладывать ли». Возвращает: Offer. Ошибок нет; нет рантайма tokio при запуске доставщика — паника
    /// tokio::spawn (адаптеры работают внутри рантайма).
    pub fn slow(
        &self,
        out: &Sender<MarketEvent>,
        key: Key,
        item: Item,
        keep: &mut Ask<impl FnOnce() -> bool>,
    ) -> Offer {
        let r = self.set.slow(out, key, item, keep);
        self.started(out, r);
        r
    }

    /// Быстрый путь, иначе медленный — с готовым ключом (keep_last.rs offer; тесты). Принимает: канал, ключ, слот,
    /// «откладывать ли». Возвращает: Offer. Ошибок нет (паника — как у slow).
    pub fn offer(
        &self,
        out: &Sender<MarketEvent>,
        key: Key,
        item: Item,
        keep: impl FnOnce() -> bool,
    ) -> Offer {
        let r = self.set.offer(out, key, item, keep);
        self.started(out, r);
        r
    }

    /// Проверка «пул годен» площадки DEX при доставке (заменяет прежнюю этой площадки). Ошибок нет.
    pub fn live(&self, venue: VenueId, f: Live) {
        self.lives
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(venue, f);
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

    /// Решение 333: очередь «ждут места» сообщений рода Money / Info этого канала (core_out_must.rs). Ошибок нет.
    pub fn must(&self) -> Arc<super::core_out_must::Queue> {
        self.must.clone()
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

    /// Итог требует доставщика (первое откладывание; слияние, когда доставщик упал — Н3) — запускает его. Ошибок нет.
    fn started(&self, out: &Sender<MarketEvent>, r: Offer) {
        if matches!(
            r,
            Offer::Deferred { start: true } | Offer::Replaced { start: true }
        ) {
            self.spawn(out.clone());
        }
    }

    /// Запускает доставщика; набор опустел — крючки классов (вне замков). Ошибок нет.
    fn spawn(&self, out: Sender<MarketEvent>) {
        let feed = self.clone();
        let check = Pools(self.lives.clone());
        tokio::spawn(async move {
            if feed.set.clone().deliver(out, check).await {
                let hooks: Vec<Hook> = lock(&feed.hooks).values().cloned().collect();
                for h in hooks {
                    h(&feed);
                }
            }
        });
    }
}
