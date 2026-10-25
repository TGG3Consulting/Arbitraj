//! Файл: venues/bsc_dex/pools/group.rs
//! Что делает: подписки logs одной пачки пулов на одном соединении WebSocket. У пачки две постоянные подписки:
//! «все события» — пулы не мельче dex.min_pool_liquidity_usd, и «только добавление ликвидности» — мелкие пулы
//! (pools/depth.rs). Состав меняется на том же соединении, без переподключения: сначала подписка с новым составом,
//! потом отписка от старой (события на стыке приходят дважды — второе отбрасывает реестр по (блок, logIndex),
//! registry.rs has_applied). Уведомления всех подписок сливаются в один поток; обрыв соединения — сообщение Closed.
//! Кто вызывает: pools/session.rs, pools/admit.rs.
//! От чего зависит: node/ws.rs (Conn, Sub), node/subs.rs (фильтры), pools/events.rs (topic0), discover.rs (Found).
//! Настройки: через node/ws.rs (адрес узла, таймауты).
//!
//! eth_subscribe / eth_unsubscribe — execution-apis subscribe.yaml#L1-L60
//! (https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml#L1-L60);
//! подписки живут, пока живо соединение — https://geth.ethereum.org/docs/interacting-with-geth/rpc/pubsub .

use alloy_primitives::{B256, U64};
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;

use super::super::node::source::Link;
use super::super::node::subs;
use super::discover::Found;
use super::events;

/// Сообщение слитого потока подписок пачки.
pub enum Msg {
    /// Лог одной из подписок (поле result уведомления).
    Log(Box<RawValue>),
    /// Соединение оборвалось — все подписки пропали.
    Closed,
}

/// Подписка узла и задача, переносящая её уведомления в общий поток.
pub struct Held {
    id: String,
    fwd: JoinHandle<()>,
}

/// Какие события слушать у пулов.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Topics {
    /// Все события, по которым ведётся состояние (events::topics).
    Full,
    /// Только добавление ликвидности (events::liquidity_topics).
    Liquidity,
}

/// Соединение пачки и её постоянные подписки.
pub struct Group {
    pub conn: Link,
    tx: mpsc::UnboundedSender<Msg>,
    pub rx: mpsc::UnboundedReceiver<Msg>,
    /// Подписка «все события» и её пулы (индексы в пачке).
    pub full: Option<(Held, Vec<usize>)>,
    /// Подписка «только ликвидность» и её пулы.
    pub liquidity: Option<(Held, Vec<usize>)>,
}

impl Group {
    /// Открывает связь пачки с источником событий (bsc.pool_events_source: WebSocket или квитанции блоков, решение 68).
    /// Ошибки: как у Link::open.
    pub async fn open(p: &super::listen::Pools, name: &str) -> GatewayResult<Group> {
        let conn = Link::open(&p.ctx, &p.feed, &p.heads, name).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        Ok(Group {
            conn,
            tx,
            rx,
            full: None,
            liquidity: None,
        })
    }

    /// Подписка на события пулов. Принимает: пулы одного вида, какие события. Ошибки: узел отказал, обрыв.
    pub async fn subscribe(&self, pools: &[&Found], topics: Topics) -> GatewayResult<Held> {
        let mut sub = self
            .conn
            .subscribe(json!(["logs", filter(pools, topics)]))
            .await?;
        let tx = self.tx.clone();
        let fwd = tokio::spawn(async move {
            while let Some(m) = sub.rx.recv().await {
                if tx.send(Msg::Log(m)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::Closed);
        });
        Ok(Held { id: sub.id, fwd })
    }

    /// Отписка: eth_unsubscribe и остановка переноса. Ошибки: узел, обрыв (поток уже закрыт).
    pub async fn release(&self, h: Held) -> GatewayResult<()> {
        h.fwd.abort();
        let _: bool = self.conn.request("eth_unsubscribe", json!([h.id])).await?;
        Ok(())
    }

    /// Блок узла WebSocket по тому же соединению: всё позже него придёт в подписки, сделанные до запроса.
    /// Ошибки: узел, обрыв.
    pub async fn block(&self) -> GatewayResult<u64> {
        let b: U64 = self.conn.request("eth_blockNumber", json!([])).await?;
        Ok(b.to())
    }

    /// Меняет состав подписки вида `topics` на `want` (индексы в `pools`): новая подписка, затем отписка от старой;
    /// пустой состав — только отписка. Тот же состав — ничего. Ошибки: узел, обрыв.
    pub async fn set(
        &mut self,
        pools: &[(Found, String)],
        want: Vec<usize>,
        topics: Topics,
    ) -> GatewayResult<()> {
        let slot = match topics {
            Topics::Full => &self.full,
            Topics::Liquidity => &self.liquidity,
        };
        if slot.as_ref().map(|(_, v)| v) == Some(&want) || (slot.is_none() && want.is_empty()) {
            return Ok(());
        }
        let new = if want.is_empty() {
            None
        } else {
            let list: Vec<&Found> = want.iter().map(|i| &pools[*i].0).collect();
            Some((self.subscribe(&list, topics).await?, want))
        };
        let old = match topics {
            Topics::Full => std::mem::replace(&mut self.full, new),
            Topics::Liquidity => std::mem::replace(&mut self.liquidity, new),
        };
        if let Some((h, _)) = old {
            self.release(h).await?;
        }
        Ok(())
    }
}

impl Drop for Group {
    /// Задачи переноса останавливаются вместе с пачкой (соединение закрывает Conn::drop).
    fn drop(&mut self) {
        for (h, _) in self.full.iter().chain(self.liquidity.iter()) {
            h.fwd.abort();
        }
    }
}

/// Фильтр logs для пулов одного вида: V2/V3 — по адресам пулов, V4 — по PoolId у менеджера. Ошибок нет.
pub fn filter(pools: &[&Found], topics: Topics) -> Value {
    let Some(first) = pools.first() else {
        return json!({});
    };
    let topic0: Vec<B256> = match topics {
        Topics::Full => events::topics(first.family),
        Topics::Liquidity => events::liquidity_topics(first.family),
    };
    match first.reference {
        PoolRef::Contract(_) => {
            let addrs: Vec<_> = pools
                .iter()
                .filter_map(|f| match f.reference {
                    PoolRef::Contract(a) => Some(a),
                    PoolRef::Singleton { .. } => None,
                })
                .collect();
            subs::by_address(&addrs, &topic0)
        }
        PoolRef::Singleton { manager, .. } => {
            let ids: Vec<_> = pools
                .iter()
                .filter_map(|f| match f.reference {
                    PoolRef::Singleton { id, .. } => Some(id),
                    PoolRef::Contract(_) => None,
                })
                .collect();
            subs::by_pool_id(manager, &ids, &topic0)
        }
    }
}
