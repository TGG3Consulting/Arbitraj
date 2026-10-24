//! Файл: venues/bsc_dex/node/failover.rs
//! Что делает: резервный провайдер узла BSC. Адреса: основной — bsc.node_http_url / bsc.node_ws_url, резервный —
//! bsc.node_http_url_backup / bsc.node_ws_url_backup (ключ — {key} и секреты bsc.node_*_key_backup). Сбой основного
//! (нет связи, таймаут, ошибка сервера 5xx, «превышен лимит» 429) — тот же запрос сразу уходит на резервный, и дальше
//! запросы идут на резервный bsc.node_backup_hold_s секунд; потом снова сначала основной. HTTP и WebSocket
//! переключаются независимо. Резервный не задан — всё как раньше, только основной.
//! Журнал: одна запись на переключение на резервный и одна на возврат к основному.
//! Кто вызывает: node/rpc.rs (HTTP), node/ws.rs (WebSocket), connection.rs (адреса для проверки).
//! От чего зависит: ctx.rs (адрес с ключом, журнал).
//! Настройки: bsc.node_http_url, bsc.node_ws_url, bsc.node_http_url_backup, bsc.node_ws_url_backup,
//! bsc.node_backup_hold_s (вся система). Секреты: bsc.node_http_key, bsc.node_ws_key, bsc.node_http_key_backup,
//! bsc.node_ws_key_backup.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;

/// Вид связи с узлом.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Link {
    Http,
    Ws,
}

/// Какой узел.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Node {
    Main,
    Backup,
}

impl Link {
    /// Настройка адреса и секрет ключа узла. Ошибок нет.
    pub fn names(self, node: Node) -> (&'static str, &'static str) {
        match (self, node) {
            (Link::Http, Node::Main) => ("bsc.node_http_url", "bsc.node_http_key"),
            (Link::Http, Node::Backup) => ("bsc.node_http_url_backup", "bsc.node_http_key_backup"),
            (Link::Ws, Node::Main) => ("bsc.node_ws_url", "bsc.node_ws_key"),
            (Link::Ws, Node::Backup) => ("bsc.node_ws_url_backup", "bsc.node_ws_key_backup"),
        }
    }

    /// Подпись для журнала. Ошибок нет.
    fn title(self) -> &'static str {
        match self {
            Link::Http => "HTTP",
            Link::Ws => "WebSocket",
        }
    }
}

/// До какого момента запросы идут на резервный (по виду связи). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Failover {
    http: Arc<Mutex<Option<Instant>>>,
    ws: Arc<Mutex<Option<Instant>>>,
}

/// Ошибка, при которой запрос уходит на резервный узел: связь, таймаут, 5xx (Unreachable), 429. Ошибок нет.
pub fn switchable(e: &GatewayError) -> bool {
    matches!(
        e.code,
        ErrorCode::Unreachable | ErrorCode::Timeout | ErrorCode::RateLimited
    )
}

/// Порядок узлов: (основной, резервный) или наоборот, пока идёт удержание; резервного без адреса нет.
/// Принимает: задан ли резервный, удержание ещё идёт. Ошибок нет.
pub fn order(backup: bool, holding: bool) -> Vec<Node> {
    match (backup, holding) {
        (false, _) => vec![Node::Main],
        (true, false) => vec![Node::Main, Node::Backup],
        (true, true) => vec![Node::Backup, Node::Main],
    }
}

impl Failover {
    fn slot(&self, link: Link) -> &Arc<Mutex<Option<Instant>>> {
        match link {
            Link::Http => &self.http,
            Link::Ws => &self.ws,
        }
    }

    /// Узлы по порядку с адресами (ключ подставлен). Резервный без адреса пропускается.
    /// Ошибки: основной не задан (NotConfigured), секрет не введён (SecretsLocked / NotConfigured).
    pub fn urls(&self, ctx: &Ctx, link: Link) -> GatewayResult<Vec<(Node, Zeroizing<String>)>> {
        let backup = ctx
            .settings
            .text(link.names(Node::Backup).0, At::default())
            .is_ok();
        let holding = self
            .slot(link)
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some_and(|t| Instant::now() < t);
        let mut out = Vec::new();
        for node in order(backup, holding) {
            let (url, key) = link.names(node);
            match ctx.node_url(url, key) {
                Ok(u) => out.push((node, u)),
                // Без основного узла не работаем; сбой с ключом резервного — только без резервного.
                Err(e) if node == Node::Main => return Err(e),
                Err(_) => {}
            }
        }
        Ok(out)
    }

    /// Основной узел не ответил: удержание на резервном на bsc.node_backup_hold_s (не задана — без удержания,
    /// только этот запрос). Запись в журнал — при начале удержания. Ошибок нет.
    pub async fn trip(&self, ctx: &Ctx, link: Link, why: &str) {
        let hold = ctx
            .settings
            .integer("bsc.node_backup_hold_s", At::default())
            .ok();
        let fresh = {
            let mut slot = self.slot(link).lock().unwrap_or_else(|p| p.into_inner());
            let fresh = slot.is_none_or(|t| Instant::now() >= t);
            *slot = hold.map(|s| Instant::now() + Duration::from_secs(s));
            fresh
        };
        if fresh {
            let msg = format!(
                "Основной узел BSC ({}) не ответил: {why} — запросы идут на резервный (bsc.node_backup_hold_s)",
                link.title()
            );
            ctx.log(Level::Warn, "venue.node", msg, None).await;
        }
    }

    /// Основной узел ответил после удержания — запись «снова на основном» один раз. Ошибок нет.
    pub async fn main_ok(&self, ctx: &Ctx, link: Link) {
        let was = self
            .slot(link)
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if was.is_some() {
            let msg = format!("Основной узел BSC ({}) снова отвечает", link.title());
            ctx.log(Level::Info, "venue.node", msg, None).await;
        }
    }
}
