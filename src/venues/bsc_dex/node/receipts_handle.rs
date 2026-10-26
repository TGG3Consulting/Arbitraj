//! Файл: venues/bsc_dex/node/receipts_handle.rs
//! Что делает: «соединение» пачки пулов с лентой квитанций (решение 68): подписка logs по фильтру, eth_blockNumber
//! (последний разобранный блок), eth_unsubscribe, причина обрыва; подписки снимаются вместе с «соединением».
//! Кто вызывает: node/source.rs (Link::Receipts), node/receipts.rs (Feed::open).
//! От чего зависит: receipts.rs (Feed, State), log_filter.rs, ws.rs (Sub).
//! Настройки: нет.

use std::sync::Mutex;

use serde_json::Value;
use tokio::sync::mpsc;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::errors;
use super::log_filter::Filter;
use super::receipts::{Feed, SOURCE, State};
use super::ws::Sub;

/// «Соединение» с лентой одной пачки пулов: свои подписки, поколение ленты на момент открытия.
pub struct Handle {
    feed: Feed,
    generation: u64,
    ids: Mutex<Vec<String>>,
    /// Номер в учёте живых «соединений» ленты (State::links).
    link: u64,
}

impl Handle {
    /// Новое «соединение» ленты поколения `generation` пачки `name`. Ошибок нет.
    pub(super) fn new(feed: Feed, generation: u64, name: &str) -> Handle {
        let link = {
            let mut s = feed.lock();
            s.next_link += 1;
            let n = s.next_link;
            s.links.insert(n, (generation, name.to_string()));
            n
        };
        Handle {
            feed,
            generation,
            ids: Mutex::new(Vec::new()),
            link,
        }
    }

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
    /// Подписки пачки снимаются вместе с ней (как подписки соединения WebSocket при его закрытии); счёт живых
    /// «соединений» поколения уменьшается.
    fn drop(&mut self) {
        let ids = std::mem::take(&mut *self.ids.lock().unwrap_or_else(|p| p.into_inner()));
        let mut s = self.feed.lock();
        for id in ids {
            s.subs.remove(&id);
        }
        // Учёт живых «соединений»: прошлого поколения закрыто — «сброс» ленты может сниматься (Feed::release_if_settled).
        s.links.remove(&self.link);
        drop(s);
        // Лента остановлена (источник сменён на ws): снять «сброс» больше некому — последнее старое «соединение»
        // снимает его само (его пачка уже сделала пулы недостоверными, pools/session.rs).
        if self.feed.source_off() {
            self.feed.release_if_settled();
        }
    }
}
