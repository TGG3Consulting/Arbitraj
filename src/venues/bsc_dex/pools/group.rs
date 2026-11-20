//! Файл: venues/bsc_dex/pools/group.rs
//! Что делает: подписки logs одной пачки пулов на одном соединении WebSocket. У пачки две постоянные подписки:
//! «все события» — пулы не мельче dex.min_pool_liquidity_usd, и «только добавление ликвидности» — мелкие пулы
//! (pools/depth.rs). Состав меняется на том же соединении, без переподключения: сначала подписка с новым составом,
//! потом отписка от старой (события на стыке приходят дважды — второе отбрасывает реестр по (блок, logIndex),
//! registry.rs has_applied). Уведомления всех подписок сливаются в один поток; обрыв соединения — сообщение Closed.
//! Решение 454: подписка «все события» пулов видов transfer_in::watched (crypto-пулы Curve с gulp балансов) — ещё и
//! второй фильтр logs на том же соединении: ERC-20 Transfer с to = адрес пула (transfer_in.rs filter); оба фильтра —
//! одна Held (создаются до eth_blockNumber приёма, снимаются вместе). У прочих видов второго фильтра нет.
//! Кто вызывает: pools/session.rs, pools/admit.rs.
//! От чего зависит: node/source.rs (Link), group_filter.rs (фильтр logs, индексы пачки), discover.rs (Found),
//! transfer_in.rs (решение 454).
//! Настройки: через node/ws.rs (адрес узла, таймауты).
//!
//! eth_subscribe / eth_unsubscribe — execution-apis subscribe.yaml#L1-L60
//! (https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml#L1-L60);
//! подписки живут, пока живо соединение — https://geth.ethereum.org/docs/interacting-with-geth/rpc/pubsub .

use alloy_primitives::U64;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::gateway::errors::GatewayResult;

use super::super::node::source::Link;
use super::discover::Found;

/// Фильтр подписки и индексы пачки — group_filter.rs (вынесены из этого файла; прежние пути вызова сохранены).
pub use super::group_filter::{filter, index, multi_sites};

/// Сообщение слитого потока подписок пачки.
pub enum Msg {
    /// Лог одной из подписок (поле result уведомления).
    Log(Box<RawValue>),
    /// Соединение оборвалось — все подписки пропали.
    Closed,
}

/// Подписка узла и задача, переносящая её уведомления в общий поток; решение 454 — и подписка Transfer на пулы
/// (transfer_in.rs), если у пулов подписки она есть.
pub struct Held {
    id: String,
    fwd: JoinHandle<()>,
    transfers: Option<(String, JoinHandle<()>)>,
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
        let conn = Link::open(&p.ctx, &p.feed, &p.heads, &p.out, name).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        Ok(Group {
            conn,
            tx,
            rx,
            full: None,
            liquidity: None,
        })
    }

    /// Подписка на события пулов. Принимает: пулы одного вида, какие события. Решение 454: «все события» пулов видов
    /// transfer_in::watched — и подписка Transfer на их адреса (не удалась — первая снимается, ошибка наружу).
    /// Ошибки: узел отказал, обрыв.
    pub async fn subscribe(&self, pools: &[&Found], topics: Topics) -> GatewayResult<Held> {
        let (id, fwd) = self.forward(filter(pools, topics)).await?;
        let extra = match topics {
            Topics::Full => super::transfer_in::filter(pools),
            Topics::Liquidity => None,
        };
        let transfers = match extra {
            None => None,
            Some(f) => match self.forward(f).await {
                Ok(x) => Some(x),
                Err(e) => {
                    let h = Held {
                        id,
                        fwd,
                        transfers: None,
                    };
                    let _ = self.release(h).await;
                    return Err(e);
                }
            },
        };
        Ok(Held { id, fwd, transfers })
    }

    /// Подписка logs с фильтром и задача переноса её уведомлений в общий поток. Возвращает: id подписки и задачу.
    /// Ошибки: узел отказал, обрыв.
    async fn forward(&self, f: Value) -> GatewayResult<(String, JoinHandle<()>)> {
        let mut sub = self.conn.subscribe(json!(["logs", f])).await?;
        let tx = self.tx.clone();
        let fwd = tokio::spawn(async move {
            while let Some(m) = sub.rx.recv().await {
                if tx.send(Msg::Log(m)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::Closed);
        });
        Ok((sub.id, fwd))
    }

    /// Отписка: eth_unsubscribe и остановка переноса (решение 454 — и подписки Transfer). Ошибки: узел, обрыв (поток
    /// уже закрыт).
    pub async fn release(&self, h: Held) -> GatewayResult<()> {
        h.fwd.abort();
        if let Some((_, fwd)) = &h.transfers {
            fwd.abort();
        }
        let _: bool = self.conn.request("eth_unsubscribe", json!([h.id])).await?;
        if let Some((id, _)) = h.transfers {
            let _: bool = self.conn.request("eth_unsubscribe", json!([id])).await?;
        }
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
            if let Some((_, fwd)) = &h.transfers {
                fwd.abort();
            }
        }
    }
}
