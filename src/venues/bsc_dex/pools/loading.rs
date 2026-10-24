//! Файл: venues/bsc_dex/pools/loading.rs
//! Что делает: какие пулы подписки найдены и проверены, но их состояние ещё не прочитано (решение 61), и сообщение
//! ядру списка их монет (MarketEvent::PoolsLoading — полный список, только при изменении). Так ядро отличает «пулы
//! найдены, загружаются» от «пула с USDT нет». Пул уходит из списка, когда прочитан или окончательно не прочитался
//! (dex.pool_load_retries исчерпаны — дальше он «недостоверен», а не «загружается»).
//! Кто вызывает: pools/market.rs (стартовые пачки), attach.rs (найденные позже), reload.rs (прочитан / не прочитан).
//! От чего зависит: listen.rs (Pools: канал событий ядра), gateway/events.rs.
//! Настройки: нет.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use alloy_primitives::Address;
use tokio::sync::mpsc::error::TrySendError;

use crate::core::types::PoolRef;
use crate::gateway::events::MarketEvent;

use super::discover::Found;
use super::listen::Pools;

/// Пулы, ждущие первого чтения, по подписке. Дешёво клонируется; на новую подписку — новый (parts.rs).
#[derive(Clone, Default)]
pub struct Loading {
    inner: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    /// Пул → монета.
    pools: HashMap<PoolRef, Address>,
    /// Последний отправленный ядру список монет.
    sent: Option<BTreeSet<Address>>,
}

impl Loading {
    /// Пулы ждут первого чтения. Ошибок нет.
    pub fn add<'a>(&self, list: impl IntoIterator<Item = &'a Found>) {
        let mut s = self.inner.lock().unwrap_or_else(|x| x.into_inner());
        for f in list {
            s.pools.insert(f.reference.clone(), f.coin);
        }
    }

    /// Пул прочитан или окончательно не прочитался. Ошибок нет.
    pub fn done(&self, r: &PoolRef) {
        self.inner
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .pools
            .remove(r);
    }

    /// Сообщает ядру список монет, если он изменился с прошлого раза. Ошибок нет: канал полон — сообщится в
    /// следующий раз (список не помечается отправленным); закрыт — слушать некому.
    pub fn publish(&self, p: &Pools) {
        let mut s = self.inner.lock().unwrap_or_else(|x| x.into_inner());
        let now: BTreeSet<Address> = s.pools.values().copied().collect();
        if s.sent.as_ref() == Some(&now) {
            return;
        }
        let ev = MarketEvent::PoolsLoading {
            venue: p.ctx.venue(),
            coins: now.iter().copied().collect(),
        };
        match p.out.try_send(ev) {
            Ok(()) | Err(TrySendError::Closed(_)) => s.sent = Some(now),
            Err(TrySendError::Full(_)) => {}
        }
    }
}
