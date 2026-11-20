//! Файл: venues/bsc_dex/pools/ekubo_hub.rs
//! Что делает: одна подписка на логи ядра Ekubo (и его TWAMM) на всю площадку с раздачей по пачкам (решение 454,
//! техдолг 447 п. 3). У ядра PoolId не в topics, поэтому подписка пачки — «все логи ядра»; раньше каждая пачка пулов
//! Ekubo держала свою такую подписку, и узел слал КАЖДЫЙ лог ядра каждой пачке (нагрузка × число пачек). Теперь при
//! bsc.pool_events_source = ws: одно соединение и одна подписка logs {address: [ядро, TWAMM…]} на (площадка, адреса);
//! лог разбирается один раз (PoolId — ekubo_route.rs, «topic0»), пачка получает только логи своих пулов (и своих
//! «topic0» — у подписки «только ликвидность»). Хаб живёт, пока его держит хоть одна пачка (Arc); обрыв его
//! соединения или отставание пачки больше dex.ekubo_hub_buffer сообщений — пачке Closed (переподключение и
//! перечитывание, как при обрыве своей подписки). Номер блока «всё позже придёт в подписки» пачка Ekubo спрашивает у
//! соединения хаба (group.rs block) — порядок тот же, что у своей подписки. Лента квитанций (receipts) и так одна на
//! площадку — там хаб не нужен.
//! Кто вызывает: pools/group.rs.
//! От чего зависит: ctx.rs, node/ws.rs (Conn), ekubo_route.rs, group.rs (Msg).
//! Настройки: dex.ekubo_hub_buffer [площадка DEX] (не задана — пачки Ekubo не подписываются, причина — NotConfigured).
//!
//! eth_subscribe logs: address — массив адресов, topics не задан — любые логи (execution-apis 16caee27
//! src/eth/subscribe.yaml L1-L60); уведомления подписки идут по соединению по порядку с ответами — geth pubsub.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256};
use serde_json::json;
use serde_json::value::RawValue;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::ws::Conn;
use super::ekubo_route;
use super::group::Msg;

/// Лог, разобранный один раз: PoolId (None — не лог пула), «topic0» (ekubo_route::topic), сырой текст.
pub struct Item {
    pub id: Option<B256>,
    pub topic: Option<B256>,
    pub raw: Arc<str>,
}

/// Общая подписка: соединение, раздача (None — соединение оборвалось), признак обрыва, задача приёма.
pub struct Hub {
    pub conn: Conn,
    tx: broadcast::Sender<Option<Arc<Item>>>,
    closed: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl Hub {
    /// Соединение хаба оборвалось. Ошибок нет.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

impl Drop for Hub {
    /// Задача приёма останавливается вместе с хабом (соединение закрывает Conn::drop).
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Живые хабы по (площадка, адреса).
static HUBS: LazyLock<Mutex<HashMap<String, Weak<Hub>>>> = LazyLock::new(Default::default);

/// Хаб площадки для адресов (живой — общий, иначе — новый). Принимает: окружение, имя соединения, адреса подписки.
/// Ошибки: NotConfigured (dex.ekubo_hub_buffer), как у Conn::open и eth_subscribe.
pub async fn get(ctx: &Ctx, name: &str, mut addrs: Vec<Address>) -> GatewayResult<Arc<Hub>> {
    addrs.sort();
    addrs.dedup();
    let key = format!("{}:{addrs:?}", ctx.place.id);
    let live = {
        let hubs = HUBS.lock().unwrap_or_else(|p| p.into_inner());
        hubs.get(&key).and_then(Weak::upgrade)
    };
    if let Some(h) = live.filter(|h| !h.is_closed()) {
        return Ok(h);
    }
    let buffer = ctx.int("dex.ekubo_hub_buffer")?;
    let conn = Conn::open(ctx, &format!("{name} (хаб Ekubo)")).await?;
    let filter = json!(["logs", { "address": addrs }]);
    let mut sub = conn.subscribe(filter).await?;
    // Предел ёмкости канала tokio — usize::MAX / 2 (broadcast::channel), не порог.
    let cap = usize::try_from(buffer).unwrap_or(usize::MAX);
    let (tx, _) = broadcast::channel(cap.clamp(1, usize::MAX >> 1));
    let closed = Arc::new(AtomicBool::new(false));
    let (out, end) = (tx.clone(), closed.clone());
    let task = tokio::spawn(async move {
        while let Some(raw) = sub.rx.recv().await {
            let (id, topic) = match serde_json::from_str::<Log>(raw.get()) {
                Ok(l) => (ekubo_route::pool_id(&l), ekubo_route::topic(&l)),
                Err(_) => (None, None),
            };
            let item = Item {
                id,
                topic,
                raw: Arc::from(raw.get()),
            };
            let _ = out.send(Some(Arc::new(item)));
        }
        end.store(true, Ordering::Release);
        let _ = out.send(None);
    });
    let hub = Arc::new(Hub {
        conn,
        tx,
        closed,
        task,
    });
    let mut hubs = HUBS.lock().unwrap_or_else(|p| p.into_inner());
    hubs.insert(key, Arc::downgrade(&hub));
    drop(hubs);
    Ok(hub)
}

/// Раздача пачке: логи пулов `ids` (и «topic0» из `topics`, если задан) — в поток пачки; обрыв хаба или отставание —
/// Msg::Closed. Приём из хаба начинается сразу (до возврата), поэтому номер блока, спрошенный после, — граница
/// «всё позже придёт». Принимает: хаб, PoolId пулов, «topic0» (None — любые), поток пачки. Ошибок нет.
pub fn forward(
    hub: &Hub,
    ids: HashSet<B256>,
    topics: Option<Vec<B256>>,
    tx: mpsc::UnboundedSender<Msg>,
) -> JoinHandle<()> {
    let mut rx = hub.tx.subscribe();
    tokio::spawn(async move {
        loop {
            let it = match rx.recv().await {
                Ok(Some(it)) => it,
                Ok(None) | Err(RecvError::Closed) | Err(RecvError::Lagged(_)) => {
                    let _ = tx.send(Msg::Closed);
                    return;
                }
            };
            let kind = topics
                .as_ref()
                .is_none_or(|t| it.topic.is_some_and(|x| t.contains(&x)));
            if !(kind && it.id.is_some_and(|i| ids.contains(&i))) {
                continue;
            }
            let Ok(raw) = RawValue::from_string(it.raw.to_string()) else {
                continue;
            };
            if tx.send(Msg::Log(raw)).is_err() {
                return;
            }
        }
    })
}
