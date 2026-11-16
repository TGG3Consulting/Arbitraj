//! Файл: gateway/core_out_must.rs
//! Что делает: решение 333 (задача t155) — сообщения адаптеров ядру, которые раньше отдавались `let _ = out.try_send`
//! и молча терялись при полном канале событий ядра (ёмкость --live-buffer). Для ВСЕХ площадок и всех таких сообщений —
//! одна очередь «ждут места» на канал ядра (живёт в общем наборе канала, gateway/core_out.rs Feed::must) и один
//! доставщик. Два рода сообщений (Kind):
//! - Money — по ним ядро решает о деньгах: WalletQueue (очередь кошелька DEX: «остановлена» закрывает сделки биржа →
//!   DEX в этот кошелёк — core/wallet_queue_gate.rs; «снята» открывает их), ExitGap (замер выхода — закрыть монету для
//!   входа, core/exit_gap.rs). Настройка venue.money_events_keep.
//! - Info — уведомления (Notice), счётчики каналов отправки (SendChannel), «монета не проверена» (CoinCheck в начале
//!   цикла налогов). Настройка venue.info_events_keep.
//!
//! Почему очередь по порядку (FIFO), а не «последнее значение по ключу» (keep_last, решение 324): каждое такое сообщение —
//! отдельное событие, а не полное состояние, которое можно заменить свежим: смена «остановлена → свободна» даёт
//! уведомления send.queue_halted / send.queue_released по ПРЕДЫДУЩЕМУ состоянию (core/queue_notify.rs), ExitGap — замер
//! одной сделки, Notice — отдельный текст. Слияние потеряло бы их так же, как прежний try_send. Сообщения редкие (на
//! транзакцию, сделку, уведомление), очередь не растёт без предела на горячем пути.
//! Порядок: очередь не пуста — новое сообщение встаёт в её конец (не обгоняет ждущие), при любой настройке (выключение
//! действует, когда очередь опустела — как правило 321). Очередь пуста — без ожидания (try_reserve), как раньше.
//! Канал полон: настройка рода «да» — в очередь, доставщик ждёт место (reserve().await — честная очередь tokio вместе с
//! доставщиком набора keep_last, никто не голодает); не «да» (не задана) — как раньше потеряно, но не молча: счёт и
//! запись журнала venue.core_events_lost (gateway/lost_log.rs).
//! Кто вызывает: venues/bsc_dex/send/stats.rs (WalletQueue, ExitGap, Notice исполнения, SendChannel),
//! venues/bsc_dex/resubscribe_wallets.rs, venues/bsc_dex/check/{tax_cycle,tax_pass}.rs, venues/bingx/rate_pause.rs,
//! venues/gate/withdraw_notice.rs.
//! От чего зависит: gateway/core_out.rs (очередь на канал), gateway/core_out_book.rs (Fed), gateway/lost_log.rs,
//! settings, storage/journal.rs, tokio.
//! Настройки: venue.money_events_keep, venue.info_events_keep (вся система; необязательные); журнал —
//! log.events_lost_interval_s.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;

use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::Journal;

use super::core_out;
pub use super::core_out_book::Fed;
use super::events::MarketEvent;
use super::lost_log::{Lost, Note};

/// Настройка рода Money.
pub const KEEP_MONEY: &str = "venue.money_events_keep";
/// Настройка рода Info.
pub const KEEP_INFO: &str = "venue.info_events_keep";
/// Вид записи журнала о потерях.
pub const LOST_KIND: &str = "venue.core_events_lost";

/// Род сообщения (см. шапку).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Money,
    Info,
}

impl Kind {
    /// Род события: WalletQueue и ExitGap — Money, остальное — Info. Ошибок нет.
    pub fn of(ev: &MarketEvent) -> Kind {
        match ev {
            MarketEvent::WalletQueue(_) | MarketEvent::ExitGap(_) => Kind::Money,
            _ => Kind::Info,
        }
    }

    /// Имя настройки рода. Ошибок нет.
    fn setting(self) -> &'static str {
        match self {
            Kind::Money => KEEP_MONEY,
            Kind::Info => KEEP_INFO,
        }
    }
}

/// Очередь «ждут места» одного канала ядра (одна на канал, Feed::must).
#[derive(Debug, Default)]
pub struct Queue {
    state: Mutex<State>,
    /// Потери рода Money / Info (канал полон, настройка не «да»).
    lost_money: Lost,
    lost_info: Lost,
}

/// Состояние под замком.
#[derive(Debug, Default)]
struct State {
    items: VecDeque<MarketEvent>,
    /// Доставщик запущен.
    running: bool,
}

/// Замок без паники на отравлении. Ошибок нет.
fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Отправитель сообщений рода Money / Info одного адаптера. Дешёво клонируется.
#[derive(Clone)]
pub struct MustOut {
    out: Sender<MarketEvent>,
    settings: Store,
    journal: Journal,
}

impl MustOut {
    /// Принимает: канал ядра, настройки, журнал. Ошибок нет.
    pub fn new(out: Sender<MarketEvent>, settings: &Store, journal: &Journal) -> Self {
        Self {
            out,
            settings: settings.clone(),
            journal: journal.clone(),
        }
    }

    /// Канал ядра (для сравнения и прежних вызывающих). Ошибок нет.
    pub fn out(&self) -> &Sender<MarketEvent> {
        &self.out
    }

    /// Сообщение ядру (см. шапку); род — по событию (Kind::of). Возвращает: Fed. Ошибок нет; нет рантайма tokio при
    /// запуске доставщика — сообщение считается потерянным (адаптеры работают внутри рантайма).
    pub fn send(&self, ev: MarketEvent) -> Fed {
        let kind = Kind::of(&ev);
        let q = core_out::of(&self.out).must();
        let mut s = lock(&q.state);
        if !s.items.is_empty() {
            s.items.push_back(ev);
            return Fed::Kept;
        }
        let ev = match self.out.try_reserve() {
            Ok(p) => {
                p.send(ev);
                return Fed::Sent;
            }
            Err(TrySendError::Closed(())) => return Fed::Closed,
            Err(TrySendError::Full(())) => ev,
        };
        let keep = matches!(self.settings.flag(kind.setting(), At::default()), Ok(true));
        if keep && let Ok(rt) = tokio::runtime::Handle::try_current() {
            s.items.push_back(ev);
            if !s.running {
                s.running = true;
                rt.spawn(deliver(q.clone(), self.out.clone()));
            }
            return Fed::Kept;
        }
        drop(s);
        let last = match &ev {
            MarketEvent::Notice { code, .. } => format!("уведомление {code}"),
            MarketEvent::WalletQueue(w) => format!("очередь кошелька {}: {:?}", w.venue.0, w.state),
            MarketEvent::ExitGap(g) => {
                format!("замер выхода {} (сделка {})", g.asset.ticker, g.deal_id)
            }
            MarketEvent::SendChannel(c) => format!("счётчики канала отправки {}", c.id),
            MarketEvent::CoinCheck(c) => format!("проверка монеты {}", c.asset.ticker),
            _ => String::new(),
        };
        let (lost, what) = match kind {
            Kind::Money => (
                &q.lost_money,
                "канал событий ядра полон — сообщение, по которому ядро решает о деньгах, потеряно (venue.money_events_keep не «да»)",
            ),
            Kind::Info => (
                &q.lost_info,
                "канал событий ядра полон — уведомление / счётчик / проверка потеряны (venue.info_events_keep не «да»)",
            ),
        };
        let note = Note {
            kind: LOST_KIND,
            venue: None,
            what,
            last,
        };
        lost.hit(&self.settings, &self.journal, note);
        Fed::Lost
    }
}

/// Доставщик очереди: ждёт место в канале и отдаёт сообщения по порядку; очередь опустела — выходит. Канал закрыт —
/// очередь очищается (слать некому). Ошибок нет.
async fn deliver(q: Arc<Queue>, out: Sender<MarketEvent>) {
    loop {
        let Ok(permit) = out.reserve().await else {
            let mut s = lock(&q.state);
            s.items.clear();
            s.running = false;
            return;
        };
        let mut s = lock(&q.state);
        match s.items.pop_front() {
            Some(ev) => permit.send(ev),
            None => {
                s.running = false;
                return;
            }
        }
        if s.items.is_empty() {
            s.running = false;
            return;
        }
    }
}
