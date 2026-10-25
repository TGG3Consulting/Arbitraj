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

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::errors;
use super::heads::Heads;
use super::log_filter::Filter;
use super::ws::Sub;

/// Источник ошибок для текста причины.
pub const SOURCE: &str = "квитанции блоков (HTTP)";

/// Лента квитанций одной подписки на рынок. Дешёво клонируется; задача ленты кончается вместе с последней копией.
#[derive(Clone, Default)]
pub struct Feed {
    pub(super) inner: Arc<Inner>,
}

#[derive(Default)]
pub(super) struct Inner {
    pub(super) state: Mutex<State>,
    started: AtomicBool,
    /// Последний разобранный блок (None — лента ещё не начала или оборвалась).
    pub(super) last: watch::Sender<Option<u64>>,
}

/// Подписки и «поколение» ленты: обрыв — новое поколение, подписки старого закрыты.
#[derive(Default)]
pub(super) struct State {
    pub(super) subs: HashMap<String, (Filter, mpsc::UnboundedSender<Box<RawValue>>)>,
    next: u64,
    pub(super) generation: u64,
    /// Причина последнего обрыва.
    pub(super) broken: Option<String>,
}

/// «Соединение» с лентой одной пачки пулов: свои подписки, поколение ленты на момент открытия.
pub struct Handle {
    feed: Feed,
    generation: u64,
    ids: Mutex<Vec<String>>,
}

impl Feed {
    /// Открывает «соединение»; первая — запускает задачу ленты (класс Check очереди узла: события не ждут фона).
    /// Принимает: окружение, головы сети. Ошибок нет.
    pub fn open(&self, ctx: &Ctx, heads: &Heads) -> Handle {
        if !self.inner.started.swap(true, Ordering::SeqCst) {
            let weak = Arc::downgrade(&self.inner);
            let work = super::receipts_task::run(weak, ctx.clone(), heads.clone());
            tokio::spawn(crate::gateway::urgency::scope(
                crate::gateway::urgency::Urgency::Check,
                work,
            ));
        }
        let generation = self.lock().generation;
        Handle {
            feed: self.clone(),
            generation,
            ids: Mutex::new(Vec::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Handle {
    /// Подписка logs. Принимает: ["logs", фильтр]. Возвращает: Sub (поток логов). Ошибки: Unreachable — лента
    /// оборвалась после открытия; BadResponse — не подписка logs.
    pub fn subscribe(&self, params: &Value) -> GatewayResult<Sub> {
        let filter = match params.as_array().map(Vec::as_slice) {
            Some([kind, f]) if kind == "logs" => Filter::new(f),
            _ => {
                return Err(errors::err(
                    ErrorCode::BadResponse,
                    format!("{SOURCE}: только подписка logs"),
                ));
            }
        };
        let mut s = self.feed.lock();
        if s.generation != self.generation {
            return Err(errors::err(ErrorCode::Unreachable, self.reason(&s)));
        }
        s.next += 1;
        let id = format!("receipts-{}", s.next);
        let (tx, rx) = mpsc::unbounded_channel();
        s.subs.insert(id.clone(), (filter, tx));
        self.ids
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(id.clone());
        Ok(Sub { id, rx })
    }

    /// Запрос как к узлу: eth_blockNumber — последний разобранный блок (ждёт начала ленты), eth_unsubscribe — снять
    /// подписку. Ошибки: Unreachable — лента оборвалась; BadResponse — другой метод.
    pub async fn request(&self, method: &str, params: &Value) -> GatewayResult<Value> {
        match method {
            "eth_unsubscribe" => {
                let id = params.get(0).and_then(Value::as_str).unwrap_or_default();
                let had = self.feed.lock().subs.remove(id).is_some();
                Ok(Value::Bool(had))
            }
            "eth_blockNumber" => {
                let mut rx = self.feed.inner.last.subscribe();
                loop {
                    if self.end_reason().is_some() {
                        return Err(errors::err(
                            ErrorCode::Unreachable,
                            self.reason(&self.feed.lock()),
                        ));
                    }
                    if let Some(n) = *rx.borrow_and_update() {
                        return Ok(Value::String(format!("0x{n:x}")));
                    }
                    if rx.changed().await.is_err() {
                        return Err(errors::err(
                            ErrorCode::Unreachable,
                            format!("{SOURCE}: лента остановлена"),
                        ));
                    }
                }
            }
            _ => Err(errors::err(
                ErrorCode::BadResponse,
                format!("{SOURCE}: метод {method} не поддерживается"),
            )),
        }
    }

    /// Причина обрыва ленты после открытия; None — жива. Ошибок нет.
    pub fn end_reason(&self) -> Option<String> {
        let s = self.feed.lock();
        (s.generation != self.generation).then(|| self.reason(&s))
    }

    fn reason(&self, s: &State) -> String {
        s.broken
            .clone()
            .unwrap_or_else(|| format!("{SOURCE}: лента оборвалась"))
    }
}

impl Drop for Handle {
    /// Подписки пачки снимаются вместе с ней (как подписки соединения WebSocket при его закрытии).
    fn drop(&mut self) {
        let ids = std::mem::take(&mut *self.ids.lock().unwrap_or_else(|p| p.into_inner()));
        let mut s = self.feed.lock();
        for id in ids {
            s.subs.remove(&id);
        }
    }
}
