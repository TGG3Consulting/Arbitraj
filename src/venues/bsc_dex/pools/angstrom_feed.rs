//! Подписка на открытые аттестации Angstrom. Каждая подпись проходит контрактную
//! проверку на конкретном дочернем блоке перед использованием в денежном плане.
use crate::venues::bsc_dex::ctx::Ctx;
use alloy_primitives::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinHandle, time::timeout};
use tokio_tungstenite::tungstenite::Message;

// UnlockHook.beforeSwap делит data на address(bytes20(data[:20])) и произвольные bytes data[20:].
// https://github.com/SorellaLabs/angstrom/blob/462a7f2a84dfbe34b61d88d4eb2dadeeae0ce94c/contracts/src/modules/UnlockHook.sol
const NODE_ADDRESS_BYTES: usize = 20;
const METHOD: &str = "consensus_subscribeEmptyBlockAttestations";
struct Feed {
    endpoint: String,
    demand: Arc<Mutex<Instant>>,
    latest: watch::Receiver<Option<(u64, Bytes)>>,
    task: JoinHandle<()>,
}
impl Drop for Feed {
    fn drop(&mut self) {
        self.task.abort();
    }
}
type Streams = HashMap<(&'static str, &'static str), Feed>;
static STREAMS: LazyLock<Mutex<Streams>> = LazyLock::new(Mutex::default);

async fn receive(
    url: String,
    idle: Duration,
    connect: Duration,
    last: Arc<Mutex<Instant>>,
    send: watch::Sender<Option<(u64, Bytes)>>,
) -> Option<()> {
    let (mut ws, _) = timeout(connect, tokio_tungstenite::connect_async(url.as_str()))
        .await
        .ok()?
        .ok()?;
    ws.send(Message::Text(
        json!({"jsonrpc":"2.0","method":METHOD,"params":[],"id":1})
            .to_string()
            .into(),
    ))
    .await
    .ok()?;
    let mut subscription = None;
    loop {
        let msg = timeout(idle, ws.next()).await.ok()??.ok()?;
        if last.lock().ok()?.elapsed() > idle {
            return None;
        }
        let Message::Text(t) = msg else {
            continue;
        };
        let v: Value = serde_json::from_str(&t).ok()?;
        if v["id"] == 1 {
            subscription = Some(v["result"].as_str()?.to_owned());
            continue;
        }
        if v["method"] != METHOD
            || subscription.is_none()
            || v["params"]["subscription"].as_str() != subscription.as_deref()
        {
            continue;
        }
        let r = &v["params"]["result"];
        let height = r["block"].as_u64()?;
        let data: Bytes = r["data"].as_str()?.parse().ok()?;
        send.send_replace(Some((height, data)));
    }
}

/// Пустое уведомление предупреждает об ожидаемом пакете. Его приём сам по себе не открывает следующий блок.
pub(in crate::venues::bsc_dex) fn for_child(
    ctx: &Ctx,
    id: &'static str,
    parent: u64,
) -> Option<Bytes> {
    let url = ctx.dex_text("dex.angstrom_attestation_ws", id).ok()?;
    if !url.starts_with("wss://") {
        return None;
    }
    let key = (ctx.place.id, id);
    let mut feeds = STREAMS.lock().ok()?;
    feeds.retain(|_, f| !f.task.is_finished());
    if feeds.get(&key).is_none_or(|f| f.endpoint != url) {
        let idle = Duration::from_millis(ctx.int("venue.ws_silence_ms").ok()?);
        let limit = ctx.timeout().ok()?;
        let demand = Arc::new(Mutex::new(Instant::now()));
        let (send, latest) = watch::channel(None);
        let reader = receive(url.clone(), idle, limit, demand.clone(), send);
        let task = tokio::spawn(async move {
            let _ = reader.await;
        });
        feeds.insert(
            key,
            Feed {
                endpoint: url,
                demand,
                latest,
                task,
            },
        );
    }
    let f = feeds.get(&key)?;
    *f.demand.lock().ok()? = Instant::now();
    let saved = f.latest.borrow();
    let (announced, data) = saved.as_ref()?;
    candidate(*announced,parent,data).then(|| data.clone())
}

fn candidate(announced:u64,parent:u64,data:&[u8])->bool {
    (announced==parent || Some(announced)==parent.checked_add(1)) && data.len()>=NODE_ADDRESS_BYTES
}

#[cfg(test)]
mod tests {
    use super::candidate;
    #[test]
    fn node_address_is_a_prefix_and_the_contract_checks_the_signature() {
        for len in [0,19] {assert!(!candidate(9,9,&vec![1;len]));}
        for len in [20,21,85] {let data=vec![1;len]; assert!(candidate(9,9,&data) && candidate(10,9,&data)); assert!(!candidate(8,9,&data));}
    }
}
