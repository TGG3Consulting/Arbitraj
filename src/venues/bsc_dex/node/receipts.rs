//! Файл: venues/bsc_dex/node/receipts.rs
//! Что делает: источник событий пулов «квитанции блоков по HTTP» (решение 68) — вместо подписки logs WebSocket, если
//! так выбрано настройкой bsc.pool_events_source. Ведёт себя как соединение WebSocket (node/source.rs): подписка logs с
//! фильтром → поток логов; eth_blockNumber — последний разобранный блок (всё позже него придёт в подписки, сделанные до
//! запроса); eth_unsubscribe. Блоки разбирает одна задача на подписку на рынок (receipts_task.rs) — по порядку, без
//! пропусков; логи каждого блока раздаются подпискам по их фильтрам (log_filter.rs), реорг — копии логов отменённых
//! блоков с removed: true, как у подписки WebSocket. Обрыв источника (отставание, реорг глубже окна, не задана
//! настройка) = обрыв соединения: все подписки закрываются, слушатели пулов делают пулы недостоверными и подключаются
//! заново (pools/listen.rs).
//! Кто вызывает: node/source.rs (Link::Receipts), pools/listen.rs (Pools: одна лента на подписку на рынок).
//! От чего зависит: receipts_task.rs, log_filter.rs, ws.rs (Sub), heads.rs, ctx.rs.
//! Настройки: через receipts_task.rs — bsc.receipts_max_lag_blocks, bsc.receipts_reorg_depth, bsc.receipts_retry_ms.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use serde_json::value::RawValue;
use tokio::sync::{mpsc, watch};

use super::super::ctx::Ctx;
use super::heads::Heads;
use super::log_filter::Filter;
use super::receipts_chain::route;
pub use super::receipts_handle::Handle;

/// Источник ошибок для текста причины.
pub const SOURCE: &str = "квитанции блоков (HTTP)";

/// Лента квитанций одной подписки на рынок. Дешёво клонируется; задача ленты кончается вместе с последней копией.
#[derive(Clone, Default)]
pub struct Feed {
    pub(super) inner: Arc<Inner>,
}

pub(super) struct Inner {
    pub(super) state: Mutex<State>,
    pub(super) started: AtomicBool,
    /// Лента остановлена сменой источника (receipts_hold.rs stop_source): «сброс» снимает последнее старое
    /// «соединение»; новая лента (Feed::open) снова снимает его сама.
    pub(super) source_off: AtomicBool,
    /// Последний разобранный блок (None — лента ещё не начала или оборвалась).
    pub(super) last: watch::Sender<Option<u64>>,
    /// «Данные пулов отстают на N блоков» площадки (решение 72): ставит задача ленты.
    pub(super) lag: super::events_lag::EventsLag,
    /// Канал событий ядру (уведомление «лента стоит — сброс», решение 72); ставит первое Feed::open.
    pub(super) out: Mutex<Option<tokio::sync::mpsc::Sender<crate::gateway::events::MarketEvent>>>,
    /// Темп по узлам списка bsc.receipts_node_urls (решение 72).
    pub(super) nodes: Arc<super::receipts_nodes::Nodes>,
    /// Эпоха пересинхрона (решение 77): пропуск блоков будит пачки перечитать состояние пулов без разрыва подписок.
    pub(super) resync: watch::Sender<u64>,
}

impl Default for Inner {
    fn default() -> Self {
        Inner {
            state: Mutex::default(),
            started: AtomicBool::new(false),
            source_off: AtomicBool::new(false),
            last: watch::channel(None).0,
            lag: super::events_lag::EventsLag::default(),
            out: Mutex::default(),
            nodes: Arc::default(),
            resync: watch::channel(0).0,
        }
    }
}

/// Подписки и «поколение» ленты: обрыв — новое поколение, подписки старого закрыты.
#[derive(Default)]
pub(super) struct State {
    pub(super) subs: HashMap<String, (Filter, mpsc::UnboundedSender<Box<RawValue>>)>,
    pub(super) next: u64,
    pub(super) generation: u64,
    /// Причина последнего обрыва.
    pub(super) broken: Option<String>,
    /// Живые «соединения»: номер → данные пачки. Пачка закрывает своё после Invalidated своих пулов
    /// (pools/session.rs); соединения прошлых поколений держат «сброс» (release_if_settled).
    pub(super) links: std::collections::BTreeMap<u64, Link>,
    pub(super) next_link: u64,
    /// Эпоха пересинхрона на последний запрос (решение 77): «сброс» снимается, когда каждое «соединение» перечитало
    /// пулы до этой эпохи (release_if_resynced).
    pub(super) resync_epoch: u64,
}

/// Живое «соединение» пачки с лентой.
pub(super) struct Link {
    /// Поколение ленты на момент открытия.
    pub(super) generation: u64,
    /// Имя пачки (для записи «запрет держит пачка …»).
    pub(super) name: String,
    /// До какой эпохи пересинхрона пачка перечитала пулы (решение 77). Новое соединение читает текущее состояние при
    /// приёме пулов, поэтому открывается уже на текущей эпохе — не держит снятие «сброса».
    pub(super) resynced: u64,
}

impl Feed {
    /// Лента, которая ставит отметку отставания `lag` площадки (решение 72). Ошибок нет.
    pub fn with_lag(lag: super::events_lag::EventsLag) -> Feed {
        Feed {
            inner: Arc::new(Inner {
                lag,
                ..Inner::default()
            }),
        }
    }

    /// Открывает «соединение»; первая — запускает задачу ленты (класс Check очереди узла: события не ждут фона).
    /// Принимает: окружение, головы сети, канал событий ядру. Ошибок нет.
    pub fn open(
        &self,
        ctx: &Ctx,
        heads: &Heads,
        out: &tokio::sync::mpsc::Sender<crate::gateway::events::MarketEvent>,
        name: &str,
    ) -> Handle {
        {
            let mut o = self.inner.out.lock().unwrap_or_else(|p| p.into_inner());
            if o.is_none() {
                *o = Some(out.clone());
            }
        }
        if !self.inner.started.swap(true, Ordering::SeqCst) {
            self.inner.source_off.store(false, Ordering::SeqCst);
            let weak = Arc::downgrade(&self.inner);
            let work = super::receipts_task::run(weak, ctx.clone(), heads.clone());
            tokio::spawn(crate::gateway::urgency::scope(
                crate::gateway::urgency::Urgency::Check,
                work,
            ));
        }
        self.open_named(name)
    }

    /// «Соединение» без запуска задачи ленты: блоки раздаёт тот, кто вызывает deliver (задача ленты; тесты порядка
    /// раздачи и номера блока). Ошибок нет.
    pub fn open_idle(&self) -> Handle {
        self.open_named("")
    }

    /// То же с именем пачки (для записи «запрет сброса держит пачка …»). Ошибок нет.
    pub fn open_named(&self, name: &str) -> Handle {
        let generation = self.lock().generation;
        Handle::new(self.clone(), generation, name)
    }

    /// Раздаёт логи блока подпискам по фильтрам и ставит последний разобранный блок `last` ПОД ТОЙ ЖЕ блокировкой, что
    /// и подписка (Handle::subscribe): подписка, сделанная после раздачи, видит eth_blockNumber ≥ `last` и читает
    /// состояние не раньше него — логи блока не теряются между раздачей и номером. Закрытые приёмники снимаются.
    /// Принимает: логи, номер блока для eth_blockNumber. Возвращает: логи, подошедшие хоть одной подписке. Ошибок нет.
    pub fn deliver(&self, logs: &[Value], last: u64) -> Vec<Value> {
        let mut s = self.lock();
        let ids: Vec<String> = s.subs.keys().cloned().collect();
        let filters: Vec<_> = ids.iter().map(|id| &s.subs[id].0).collect();
        let (per, any) = route(&filters, logs);
        let mut gone = Vec::new();
        for (id, idx) in ids.iter().zip(per) {
            let tx = &s.subs[id].1;
            for i in idx {
                let Ok(raw) = serde_json::value::to_raw_value(&logs[i]) else {
                    continue;
                };
                if tx.send(raw).is_err() {
                    gone.push(id.clone());
                    break;
                }
            }
        }
        for id in gone {
            s.subs.remove(&id);
        }
        self.inner.last.send_replace(Some(last));
        any
    }

    /// Обрыв ленты: новое поколение, все подписки закрыты (их приёмники видят конец), последнего блока нет. Ошибок нет.
    pub fn cut(&self, why: &str) {
        let mut s = self.lock();
        s.generation += 1;
        s.broken = Some(format!("{SOURCE}: {why}"));
        s.subs.clear();
        self.inner.last.send_replace(None);
    }

    /// Задача ленты кончилась (источник сменён): следующая Feed::open запустит её снова. Ошибок нет.
    pub(super) fn stopped(&self) {
        self.inner.started.store(false, Ordering::SeqCst);
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}
