//! Файл: venues/bsc_dex/node/source.rs
//! Что делает: источник событий пулов по настройке bsc.pool_events_source (решение 68): ws — соединение WebSocket с
//! подписками logs (node/ws.rs, как было), receipts — лента квитанций блоков по HTTP (node/receipts.rs). Для слушателей
//! пулов оба выглядят одинаково: подписка logs, eth_blockNumber «всё позже придёт в подписки», eth_unsubscribe, причина
//! обрыва — разбор событий общий (pools/session.rs, listen_log.rs, created.rs), не дублируется.
//! Кто вызывает: pools/group.rs, pools/created.rs.
//! От чего зависит: ws.rs, receipts.rs, heads.rs, ctx.rs.
//! Настройки: bsc.pool_events_source (не задана — источника нет: NotConfigured, пулы не слушаются).

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::errors;
use super::heads::Heads;
use super::receipts::{Feed, Handle};
use super::ws::{Conn, Sub};

/// Связь с источником событий пулов.
pub enum Link {
    /// WebSocket узла: подписки logs у узла.
    Ws(Conn),
    /// Квитанции блоков по HTTP: подписки — у ленты.
    Receipts(Handle),
}

impl Link {
    /// Открывает связь по bsc.pool_events_source. Принимает: окружение, ленту квитанций подписки на рынок, головы
    /// сети, канал событий ядру (уведомления ленты), имя (журнал, сырые сообщения WS). Ошибки: NotConfigured (настройка не задана или неизвестное значение),
    /// как у Conn::open.
    pub async fn open(
        ctx: &Ctx,
        feed: &Feed,
        heads: &Heads,
        out: &tokio::sync::mpsc::Sender<crate::gateway::events::MarketEvent>,
        name: &str,
    ) -> GatewayResult<Link> {
        let kind = ctx.text("bsc.pool_events_source")?;
        match kind.as_str() {
            "ws" => Ok(Link::Ws(Conn::open(ctx, name).await?)),
            "receipts" => Ok(Link::Receipts(feed.open(ctx, heads, out))),
            other => Err(ctx.not_set(
                "bsc.pool_events_source",
                format!("bsc.pool_events_source = {other}: ожидается ws или receipts"),
            )),
        }
    }

    /// Запрос с ответом (eth_blockNumber, eth_unsubscribe). Ошибки: как у источника; ответ не того вида — BadResponse.
    pub async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> GatewayResult<T> {
        match self {
            Link::Ws(c) => c.request(method, params).await,
            Link::Receipts(h) => {
                let v = h.request(method, &params).await?;
                serde_json::from_value(v)
                    .map_err(|e| errors::err(ErrorCode::BadResponse, format!("{method}: {e}")))
            }
        }
    }

    /// Подписка ["logs", фильтр]. Ошибки: как у источника.
    pub async fn subscribe(&self, params: Value) -> GatewayResult<Sub> {
        match self {
            Link::Ws(c) => c.subscribe(params).await,
            Link::Receipts(h) => h.subscribe(&params),
        }
    }

    /// Связь уже не через выбранный источник: bsc.pool_events_source сменили (или сняли) — пачке пора переподключиться
    /// через новый источник (решение 68, переключение на лету). Ошибок нет.
    pub fn stale(&self, ctx: &Ctx) -> bool {
        let kind = match self {
            Link::Ws(_) => "ws",
            Link::Receipts(_) => "receipts",
        };
        ctx.text("bsc.pool_events_source").ok().as_deref() != Some(kind)
    }

    /// Причина обрыва; None — связь жива. Ошибок нет.
    pub fn end_reason(&self) -> Option<String> {
        match self {
            Link::Ws(c) => c.end_reason(),
            Link::Receipts(h) => h.end_reason(),
        }
    }
}
