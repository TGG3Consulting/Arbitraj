//! Файл: gateway/core_out_must.rs
//! Что делает: решение 333 (задача t155) и его ревью — сообщения адаптеров ядру, которые раньше отдавались
//! `let _ = out.try_send` и молча терялись при полном канале событий ядра (ёмкость --live-buffer). Для ВСЕХ площадок —
//! очередь «ждут места» на канал ядра (gateway/core_out_must_queue.rs, живёт в общем наборе канала — core_out.rs
//! Feed::must) и один доставщик. Два рода сообщений (Kind::of — явный список всех вариантов MarketEvent):
//! - Money — по ним ядро решает о деньгах: WalletQueue («остановлена» закрывает сделки биржа → DEX в этот кошелёк —
//!   core/wallet_queue_gate.rs), ExitGap (закрыть монету для входа — core/exit_gap.rs) и др. Ревью 333, Б1: НЕ
//!   ТЕРЯЮТСЯ НИКОГДА, без настройки (как Invalidated в решении 324): потеря «остановлена» ведёт к сделке в
//!   остановленный кошелёк. Своя очередь, ВПЕРЕДИ Info (ревью, Н1): доставщик берёт сначала Money.
//! - Info — уведомления (Notice), счётчики каналов отправки (SendChannel), «монета не проверена» (CoinCheck).
//!   Ждут места по порядку всегда (решение 362, в логику: была venue.info_events_keep) — не больше venue.info_events_max
//!   (не задана — без предела); сверх предела — потеря, но не молча: счёт и запись журнала venue.core_events_lost.
//!
//! Порядок одного рода: очередь рода не пуста — новое встаёт в её конец. Очередь рода пуста —
//! без ожидания (try_reserve), как раньше. Money может обогнать ждущие Info (приоритет), наоборот — нет.
//! Ревью 333, Б2: CoinCheck одной монеты шлётся ОДНИМ путём — send_in_task (без очереди: send().await той же задачей,
//! что и итоги проверки), иначе итог мог прийти раньше «не проверена».
//! Кто вызывает: venues/bsc_dex/send/stats.rs (WalletQueue, ExitGap, Notice исполнения, SendChannel),
//! venues/bsc_dex/resubscribe_wallets.rs, venues/bsc_dex/check/{tax_cycle,tax_pass}.rs, venues/bingx/rate_pause.rs,
//! venues/gate/withdraw_notice.rs; тесты.
//! От чего зависит: gateway/core_out.rs, gateway/core_out_must_queue.rs, gateway/core_out_must_deliver.rs,
//! gateway/core_out_must_log.rs,
//! gateway/core_out_book.rs (Fed), settings, storage/journal.rs, tokio.
//! Настройки: venue.info_events_max, venue.money_events_alert_ms (вся система;
//! необязательные); журнал — log.events_lost_interval_s.

use tokio::sync::mpsc::Sender;

use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::Journal;

use super::core_out;
pub use super::core_out_book::Fed;
use super::core_out_must_deliver::deliver;
use super::core_out_must_log as log;
pub use super::core_out_must_log::{ALERT_MS, DELAYED_CODE, LOST_KIND, QUEUE_KIND};
use super::core_out_must_queue::Put;
pub use super::core_out_must_queue::{Queue, Snapshot};
use super::events::MarketEvent;

/// Предел очереди рода Info (сообщений); не задана — без предела.
pub const INFO_MAX: &str = "venue.info_events_max";

/// Род сообщения (см. шапку).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Money,
    Info,
}

impl Kind {
    /// Род события. Ревью 333, Н7: явный список ВСЕХ вариантов (без `_`): новый вариант не станет Info молча —
    /// компилятор потребует решить. Money — потеря ведёт к решению о деньгах по неверному состоянию; Info — нет.
    /// Book, Pool, Invalidated, PoolCheck, SpotPairs, Balance, CheckPass, PoolsLoading сюда сейчас не приходят (у них
    /// свои пути: 321, 324, `send().await`) — род задан по смыслу на случай, если придут. Ошибок нет.
    pub fn of(ev: &MarketEvent) -> Kind {
        match ev {
            MarketEvent::WalletQueue(_)
            | MarketEvent::ExitGap(_)
            | MarketEvent::Balance(_)
            | MarketEvent::Invalidated { .. }
            | MarketEvent::PoolCheck(_)
            | MarketEvent::SpotPairs { .. } => Kind::Money,
            MarketEvent::Notice { .. }
            | MarketEvent::SendChannel(_)
            | MarketEvent::CoinCheck(_)
            | MarketEvent::CheckPass { .. }
            | MarketEvent::PoolsLoading { .. }
            | MarketEvent::Book(_)
            | MarketEvent::Pool(_) => Kind::Info,
        }
    }
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

    /// Очередь «ждут места» канала (счёт и глубина — для журнала и тестов). Ошибок нет.
    pub fn queue(&self) -> std::sync::Arc<Queue> {
        core_out::of(&self.out).must()
    }

    /// Сообщение ядру без ожидания (см. шапку); род — Kind::of. Возвращает: Fed (Kept — ждёт в очереди). Ошибок нет;
    /// нет рантайма tokio, когда нужен доставщик, — потеря со счётом (адаптеры работают внутри рантайма).
    pub fn send(&self, ev: MarketEvent) -> Fed {
        let kind = Kind::of(&ev);
        let q = self.queue();
        let rt = tokio::runtime::Handle::try_current().ok();
        let max = || self.settings.integer(INFO_MAX, At::default()).ok();
        match q.put(&self.out, ev, kind, rt.is_some(), max) {
            Ok(Put::Sent) => Fed::Sent,
            Ok(Put::Closed) => Fed::Closed,
            Ok(Put::Kept { start }) => {
                if start && let Some(rt) = rt {
                    let (s, j) = (self.settings.clone(), self.journal.clone());
                    rt.spawn(deliver(q.clone(), self.out.clone(), s, j));
                }
                log::note(&q, &self.settings, &self.journal);
                Fed::Kept
            }
            Err(lost) => {
                let (why, ev) = *lost;
                log::lost(&q, &self.settings, &self.journal, kind, why, &ev);
                Fed::Lost
            }
        }
    }

    /// Ревью 333, Б2: сообщение рода Info, которое должно идти ОДНИМ путём с другими сообщениями той же задачи (CoinCheck
    /// «не проверена» и итог проверки монеты — check/tax_cycle.rs и вызываемые им): очередь не используется —
    /// `send().await` (ждёт места той же задачей — порядок с её следующими `send().await` сохраняется; решение 362: всегда,
    /// не теряется). Возвращает: Fed — Sent или Closed (Kept и Lost не бывает). Ошибок нет.
    pub async fn send_in_task(&self, ev: MarketEvent) -> Fed {
        match self.out.send(ev).await {
            Ok(()) => Fed::Sent,
            Err(_) => Fed::Closed,
        }
    }
}
