//! Файл: venues/bsc_dex/pools/loading.rs
//! Что делает: какие пулы подписки найдены и проверены, но их состояние ещё не прочитано (решение 61), и сообщение
//! ядру списка их монет (MarketEvent::PoolsLoading — полный список, только при изменении). Так ядро отличает «пулы
//! найдены, загружаются» от «пула с USDT нет». Пул уходит из списка, когда прочитан или окончательно не прочитался
//! (dex.pool_load_retries исчерпаны — дальше он «недостоверен», а не «загружается»); исключённый из сделок (ban) не
//! считается. Новая подписка (пулы сняты с подписки, resubscribe.rs) — новый список (parts.rs: Pools на подписку),
//! его первая отправка заменяет прежний у ядра.
//! Кто вызывает: pools/market.rs (стартовые пачки), attach.rs (найденные позже), reload.rs (прочитан / не прочитан).
//! От чего зависит: listen.rs (Pools: канал событий ядра), gateway/events.rs.
//! Настройки: нет.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use alloy_primitives::Address;

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
    /// Пул → (монета, «застрял»: ждёт незаданную настройку и не читается, пока её не зададут).
    pools: HashMap<PoolRef, (Address, bool)>,
    /// Последний отправленный ядру список монет.
    sent: Option<BTreeSet<Address>>,
}

impl Loading {
    /// Пулы ждут первого чтения. Ошибок нет.
    pub fn add<'a>(&self, list: impl IntoIterator<Item = &'a Found>) {
        let mut s = self.inner.lock().unwrap_or_else(|x| x.into_inner());
        for f in list {
            s.pools.insert(f.reference.clone(), (f.coin, false));
        }
    }

    /// Сколько пулов монеты ещё ждут первого чтения (исключённые из сделок не считаются). Принимает: окружение,
    /// адрес монеты. Ошибок нет.
    pub fn pending(&self, p: &Pools, coin: Address) -> usize {
        let s = self.inner.lock().unwrap_or_else(|x| x.into_inner());
        s.pools
            .iter()
            .filter(|(r, (c, _))| *c == coin && p.ctx.index.banned(r).is_none())
            .count()
    }

    /// Все ли ждущие пулы монеты «застряли» (ждут незаданную настройку). Нет ждущих — false. Такая монета сводку
    /// первого прохода не держит (check/tax_wait.rs), монета с реально читающимися пулами — держит. Ошибок нет.
    pub fn stuck_only(&self, p: &Pools, coin: Address) -> bool {
        let s = self.inner.lock().unwrap_or_else(|x| x.into_inner());
        let mut mine = s
            .pools
            .iter()
            .filter(|(r, (c, _))| *c == coin && p.ctx.index.banned(r).is_none())
            .peekable();
        mine.peek().is_some() && mine.all(|(_, (_, stuck))| *stuck)
    }

    /// Читается ли сейчас хоть один пул (ждёт первого чтения, не застрял, не исключён из сделок). Пока читается —
    /// сводки первого прохода сверки с квотером и крючков не отправляются (check/fresh.rs). Ошибок нет.
    pub fn reading(&self, p: &Pools) -> bool {
        let s = self.inner.lock().unwrap_or_else(|x| x.into_inner());
        s.pools
            .iter()
            .any(|(r, (_, stuck))| !*stuck && p.ctx.index.banned(r).is_none())
    }

    /// Пул «застрял» (ждёт незаданную настройку) или снова читается. Флаг сменился — сигнал «пул готов»: сводки
    /// первого прохода (налоги — check/tax_wait.rs, квотер и крючки — check/fresh.rs) пересчитываются сразу, а не по
    /// таймеру цикла. Пула нет в списке — ничего. Ошибок нет.
    pub fn stuck(&self, p: &Pools, r: &PoolRef, stuck: bool) {
        let changed = match self
            .inner
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .pools
            .get_mut(r)
        {
            Some(e) if e.1 != stuck => {
                e.1 = stuck;
                true
            }
            _ => false,
        };
        if changed {
            p.signals.pool_ready();
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

    /// Сообщает ядру список монет, если он изменился с прошлого раза. Пулы, исключённые из сделок проверкой «кто
    /// меняет» (directory ban), не считаются: они больше не грузятся для торговли. Канал полон — ждём места
    /// (send().await), отправка не теряется. Ошибок нет: канал закрыт — слушать некому.
    pub async fn publish(&self, p: &Pools) {
        let now: BTreeSet<Address> = {
            let s = self.inner.lock().unwrap_or_else(|x| x.into_inner());
            s.pools
                .iter()
                .filter(|(r, _)| p.ctx.index.banned(r).is_none())
                .map(|(_, (c, _))| *c)
                .collect()
        };
        if self
            .inner
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .sent
            .as_ref()
            == Some(&now)
        {
            return;
        }
        let ev = MarketEvent::PoolsLoading {
            venue: p.ctx.venue(),
            coins: now.iter().copied().collect(),
        };
        // Пулы монет дочитались (или брошены) — монеты, ждущие своих пулов, проверяются сразу (check/tax_cycle.rs).
        p.signals.pool_ready();
        // Закрытый канал — ядру больше не нужно; список всё равно считается отправленным.
        let _ = p.out.send(ev).await;
        self.inner.lock().unwrap_or_else(|x| x.into_inner()).sent = Some(now);
    }
}
