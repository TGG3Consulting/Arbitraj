//! Отдельные последовательные SockJS/STOMP-сессии личных заявок и фоновых ставок на плательщика. Публикации сопоставляются с подпиской.
//! https://github.com/consenlabs/tokenlon-v5-sdk · подписки Quoter, X-SEND-BY-RELAYER=false.
use super::{Failure, Query, Stage, auth, pmm_order};
use crate::{
    gateway::{errors::GatewayResult, http::Limits},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::signers::local::PrivateKeySigner;
use alloy_primitives::Address;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::Instant,
};
use tokio::{net::TcpStream, sync::Mutex as AsyncMutex, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Slot = Arc<AsyncMutex<Option<Session>>>;
type Sessions = HashMap<(&'static str, Address, bool), (Instant, Slot)>;
static SESSIONS: LazyLock<Mutex<Sessions>> = LazyLock::new(Default::default);

// Один дополнительный слот сохраняется за firm на всю площадку, свободные фоновые слоты доступны firm.
type Active = HashMap<&'static str,(usize,usize)>;
static ACTIVE: LazyLock<Mutex<Active>> = LazyLock::new(Default::default);

struct Permit(&'static str,bool);
impl Permit {
    fn reserve(counts:&mut (usize,usize),background:usize,firm:bool)->bool {
        let total=counts.0.saturating_add(counts.1);
        if total>=background.saturating_add(1) || (!firm && counts.0>=background) {return false;}
        if firm {counts.1+=1;} else {counts.0+=1;}
        true
    }
    fn acquire(ctx:&Ctx,firm:bool)->Result<Self,Failure> {
        let limit=super::rate_parallelism(ctx);
        let mut active=ACTIVE.lock().unwrap_or_else(|e|e.into_inner());
        Self::reserve(active.entry(ctx.place.id).or_default(),limit,firm)
            .then(||Self(ctx.place.id,firm)).ok_or(Failure::Throttle)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut active=ACTIVE.lock().unwrap_or_else(|e|e.into_inner());
        if let Some(counts)=active.get_mut(self.0) {
            if self.1 {counts.1-=1;} else {counts.0-=1;}
        }
    }
}

struct Session {
    stream: Stream,
    sequence: u64,
}
impl Session {
    async fn open(ctx: &Ctx, key: &PrivateKeySigner, lim: Limits) -> Result<Self, Failure> {
        let url = auth::websocket(ctx, key, lim).await?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(lim.max_bytes))
            .max_frame_size(Some(lim.max_bytes));
        let (stream, _) = connect_async_with_config(url.as_str(), Some(config), false)
            .await
            .map_err(|_| Failure::Transport)?;
        let mut s = Self {
            stream,
            sequence: 0,
        };
        if s.packet().await? != "o" {
            return Err(Failure::BadMessage);
        }
        s.send("CONNECT\naccept-version:1.1,1.0\nheart-beat:0,0\n\n\0")
            .await?;
        loop {
            if s.frames()
                .await?
                .iter()
                .any(|frame| frame.starts_with("CONNECTED\n"))
            {
                return Ok(s);
            }
        }
    }
    async fn send(&mut self, frame: &str) -> Result<(), Failure> {
        let text = serde_json::to_string(&[frame]).map_err(|_| Failure::BadMessage)?;
        self.stream
            .send(Message::Text(text.into()))
            .await
            .map_err(|_| Failure::Transport)
    }
    async fn packet(&mut self) -> Result<String, Failure> {
        loop {
            match self.stream.next().await {
                Some(Ok(Message::Text(text))) => return Ok(text.to_string()),
                Some(Ok(Message::Pong(_))) => (),
                Some(Ok(Message::Ping(_))) => {
                    self.stream.flush().await.map_err(|_| Failure::Transport)?
                }
                _ => return Err(Failure::Transport),
            }
        }
    }
    async fn frames(&mut self) -> Result<Vec<String>, Failure> {
        let packet = self.packet().await?;
        if packet == "h" {
            return Ok(Vec::new());
        }
        let body = packet.strip_prefix('a').ok_or(Failure::Transport)?;
        let frames: Vec<String> = serde_json::from_str(body).map_err(|_| Failure::BadMessage)?;
        if frames.iter().any(|f| f.starts_with("ERROR\n")) {
            return Err(Failure::Transport);
        }
        Ok(frames)
    }
    async fn read(&mut self, topic: &str) -> Result<Value, Failure> {
        self.sequence = self.sequence.checked_add(1).ok_or(Failure::Transport)?;
        let id = format!("q{}", self.sequence);
        self.send(&format!("SUBSCRIBE\nid:{id}\ndestination:{topic}\nX-TOKENLON-VERSION:5.2.0\nX-DEVICE-TOKEN:tokenlon-v5-sdk\nX-SEND-BY-RELAYER:false\nX-STRATEGY:PMMV5\n\n\0")).await?;
        loop {
            for raw in self.frames().await? {
                if let Some(value) = publication(&raw, &id)? {
                    self.send(&format!("UNSUBSCRIBE\nid:{id}\n\n\0")).await?;
                    return Ok(value);
                }
            }
        }
    }
}
pub(super) fn publication(raw: &str, id: &str) -> Result<Option<Value>, Failure> {
    let Some(message) = raw.strip_prefix("MESSAGE\n") else {
        return Ok(None);
    };
    let (headers, body) = message.split_once("\n\n").ok_or(Failure::BadMessage)?;
    let mut ids = headers
        .lines()
        .filter_map(|s| s.strip_prefix("subscription:"));
    let found = ids.next().ok_or(Failure::BadMessage)?;
    if ids.next().is_some() {
        return Err(Failure::BadMessage);
    }
    if found != id {
        return Ok(None);
    }
    let body = body.strip_suffix('\0').ok_or(Failure::BadMessage)?;
    if body == "None" {
        return Ok(None);
    }
    serde_json::from_str(body)
        .map(Some)
        .map_err(|_| Failure::BadMessage)
}

pub(super) async fn request(
    ctx: &Ctx,
    key: &PrivateKeySigner,
    q: &Query,
    firm: bool,
) -> GatewayResult<Value> {
    if !super::super::publisher_network(ctx.chain_id()) || !auth::allowed(&ctx.text("tokenlon.publisher_url")?) {
        return Err(Failure::Endpoint.error(ctx));
    }
    let owner = key.address();
    let topic = |stage| q.topic(stage, owner).ok_or(Failure::Query);
    let initial = topic(if firm { Stage::Order } else { Stage::Rate }).map_err(|e| e.error(ctx))?;
    let limits = ctx.limits()?;
    let slot = {
        let mut peers = SESSIONS.lock().unwrap_or_else(|p| p.into_inner());
        peers
            .retain(|_, (last, slot)| last.elapsed() < limits.total || Arc::strong_count(slot) > 1);
        let pair = peers
            .entry((ctx.place.id, owner, firm))
            .or_insert_with(|| (Instant::now(), Arc::default()));
        pair.0 = Instant::now();
        Arc::clone(&pair.1)
    };
    timeout(limits.total, async {
        let mut cached = if firm {
            slot.try_lock().map_err(|_|Failure::Throttle)?
        } else {slot.lock().await};
        let _permit=Permit::acquire(ctx,firm)?;
        // После отмены запроса соединение с незавершённой подпиской закрывается.
        let mut session = match cached.take() {
            Some(s) => s,
            None => Session::open(ctx, key, limits).await?,
        };
        let first = session.read(&initial).await?;
        let result = if firm {
            let now = chrono::Utc::now().timestamp().max(0) as u64;
            match pmm_order(first, owner, q, now, false) {
                Ok(_) => session.read(&topic(Stage::LastOrder)?).await,
                Err(e) => Err(e),
            }
        } else {
            Ok(first)
        };
        if result.is_ok() || matches!(&result, Err(Failure::Balance | Failure::NoOffer)) {
            *cached = Some(session);
        }
        result
    })
    .await
    .map_err(|_| Failure::Timeout.error(ctx))?
    .map_err(|e| e.error(ctx))
}

#[cfg(test)]
mod tests {
    use super::Permit;
    #[test]
    fn saturated_monitor_leaves_one_slot_for_firm_and_firm_can_borrow_idle_slots() {
        let mut active=(0,0);
        assert!(Permit::reserve(&mut active,2,false));
        assert!(Permit::reserve(&mut active,2,false));
        assert!(!Permit::reserve(&mut active,2,false));
        assert!(Permit::reserve(&mut active,2,true));
        assert!(!Permit::reserve(&mut active,2,true));
        active.0-=1;
        assert!(Permit::reserve(&mut active,2,true));
        assert!(!Permit::reserve(&mut active,2,false));
    }
}
