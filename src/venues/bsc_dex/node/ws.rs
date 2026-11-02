//! Файл: venues/bsc_dex/node/ws.rs
//! Что делает: одно соединение WebSocket с узлом BSC — открыть, запрос с ответом, подписка eth_subscribe.
//! Подписки привязаны к соединению: при обрыве все они пропадают (Geth pubsub:
//! «Subscriptions are coupled to a connection… If the connection is closed all subscriptions … are removed» —
//! https://geth.ethereum.org/docs/interacting-with-geth/rpc/pubsub). Поэтому обрыв = закрытие приёмников всех
//! подписок; владелец подписки сам переподключается (пауза venue.ws_reconnect_pause_ms) и перечитывает состояние.
//! Кто вызывает: node/heads.rs, pools/events_* (1b), connection.rs (проверка подключения).
//! От чего зависит: ctx.rs, node/ws_task.rs (цикл чтения и записи), tokio-tungstenite.
//! Настройки: bsc.node_ws_url (+ секрет bsc.node_ws_key при {key}), http.timeout_ms (ожидание ответа узла
//! и подключения), venue.ws_silence_ms [bsc_dex] (тишина = обрыв), venue.ws_ping_ms [bsc_dex] (свой Ping, решение 98).
//!
//! Методы: eth_subscribe / eth_unsubscribe, уведомления eth_subscription {subscription, result} —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml#L1-L60

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::value::RawValue;
use tokio::sync::{mpsc, oneshot};

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::errors;
use super::failover::{self, Link, Node};
use super::jsonrpc;
use super::ws_task::{self, Cmd};

/// Источник ошибок для текста причины.
pub const SOURCE: &str = "узел BSC (WebSocket)";
/// Имя секрета ключа провайдера для адреса WebSocket.
pub const SECRET_WS_KEY: &str = "bsc.node_ws_key";

/// Подписка: id от узла и поток уведомлений (поле result). Поток закрылся — соединение оборвалось.
pub struct Sub {
    pub id: String,
    pub rx: mpsc::UnboundedReceiver<Box<RawValue>>,
}

/// Соединение с узлом. При удалении задача соединения останавливается.
pub struct Conn {
    cmd: mpsc::UnboundedSender<Cmd>,
    /// Причина окончания соединения (для журнала); None — ещё живо.
    end: Arc<Mutex<Option<String>>>,
    task: tokio::task::JoinHandle<()>,
    timeout: Duration,
}

impl Conn {
    /// Открывает соединение с основным узлом, при сбое подключения — с резервным (node/failover.rs).
    /// Принимает: окружение, имя соединения (для журнала и сырых сообщений).
    /// Возвращает: соединение. Ошибки: NotConfigured/SecretsLocked (адрес, ключ, таймауты), Timeout, Unreachable.
    pub async fn open(ctx: &Ctx, name: &str) -> GatewayResult<Conn> {
        let urls = ctx.failover.urls(ctx, Link::Ws)?;
        let n = urls.len();
        let mut last = None;
        for (i, (node, url)) in urls.into_iter().enumerate() {
            match Self::connect(ctx, name, &url).await {
                Ok(c) => {
                    if node == Node::Main {
                        ctx.failover.main_ok(ctx, Link::Ws).await;
                    }
                    return Ok(c);
                }
                Err(e) if failover::switchable(&e) && i + 1 < n => {
                    if node == Node::Main {
                        ctx.failover.trip(ctx, Link::Ws, &e.message).await;
                    }
                    last = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| ctx.not_set("bsc.node_ws_url", "нет адреса узла WebSocket")))
    }

    /// Соединение по заданному адресу (основной или резервный — решает open, проверка подключения — сама).
    /// Принимает: окружение, имя, адрес с ключом. Возвращает: соединение. Ошибки: таймауты не заданы, Timeout, Unreachable.
    pub async fn connect(ctx: &Ctx, name: &str, url: &str) -> GatewayResult<Conn> {
        let timeout = ctx.timeout()?;
        let silence = Duration::from_millis(ctx.int("venue.ws_silence_ms")?);
        // Решение 98: свой Ping раз в venue.ws_ping_ms — живость тихого соединения (не задано — без Ping, как раньше).
        let ping = ctx.int("venue.ws_ping_ms").ok().map(Duration::from_millis);
        let connect = tokio::time::timeout(timeout, tokio_tungstenite::connect_async(url)).await;
        let ws = match connect {
            Err(_) => {
                return Err(ctx.err(
                    ErrorCode::Timeout,
                    format!("{SOURCE}: не подключился за {} мс", timeout.as_millis()),
                ));
            }
            // Текст ошибки tungstenite не содержит адреса; ключ из адреса в ошибку не попадает.
            Ok(Err(e)) => {
                return Err(ctx.err(
                    ErrorCode::Unreachable,
                    format!("{SOURCE}: подключение: {e}"),
                ));
            }
            Ok(Ok((ws, _))) => ws,
        };
        let (cmd, rx) = mpsc::unbounded_channel();
        let end = Arc::new(Mutex::new(None));
        let task = tokio::spawn(ws_task::run(
            ctx.clone(),
            name.to_string(),
            ws,
            rx,
            silence,
            ping,
            end.clone(),
        ));
        Ok(Conn {
            cmd,
            end,
            task,
            timeout,
        })
    }

    /// Запрос с ответом по этому соединению.
    /// Принимает: метод, параметры. Возвращает: result нужного типа.
    /// Ошибки: Timeout (нет ответа за http.timeout_ms), Unreachable (соединение закрыто), ответ-ошибка узла.
    pub async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> GatewayResult<T> {
        let (reply, rx) = oneshot::channel();
        self.send(Cmd::Request {
            method: method.to_string(),
            params,
            reply,
        })?;
        let raw = self.wait(rx).await?;
        jsonrpc::decode(&raw, SOURCE)
    }

    /// Подписка eth_subscribe.
    /// Принимает: параметры (например ["newHeads"] или ["logs", {фильтр}]).
    /// Возвращает: Sub. Ошибки: как у request (узел отказал в подписке — его текст).
    pub async fn subscribe(&self, params: Value) -> GatewayResult<Sub> {
        let (events, rx) = mpsc::unbounded_channel();
        let (reply, ack) = oneshot::channel();
        self.send(Cmd::Subscribe {
            params,
            events,
            reply,
        })?;
        let raw = self.wait(ack).await?;
        let id: String = jsonrpc::decode(&raw, SOURCE)?;
        Ok(Sub { id, rx })
    }

    /// Причина окончания соединения; None — соединение живо. Ошибок нет.
    pub fn end_reason(&self) -> Option<String> {
        self.end.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Отдаёт команду задаче соединения. Ошибка Unreachable — задача уже закончилась.
    fn send(&self, c: Cmd) -> GatewayResult<()> {
        self.cmd.send(c).map_err(|_| self.closed())
    }

    /// Ждёт ответ задачи не дольше таймаута. Ошибки: Timeout, Unreachable, ответ-ошибка узла.
    async fn wait(
        &self,
        rx: oneshot::Receiver<GatewayResult<Box<RawValue>>>,
    ) -> GatewayResult<Box<RawValue>> {
        match tokio::time::timeout(self.timeout, rx).await {
            Err(_) => Err(errors::err(
                ErrorCode::Timeout,
                format!("{SOURCE}: нет ответа за {} мс", self.timeout.as_millis()),
            )),
            Ok(Err(_)) => Err(self.closed()),
            Ok(Ok(r)) => r,
        }
    }

    /// Ошибка «соединение закрыто» с причиной.
    fn closed(&self) -> GatewayError {
        let why = self
            .end_reason()
            .unwrap_or_else(|| "соединение закрыто".into());
        errors::err(ErrorCode::Unreachable, format!("{SOURCE}: {why}"))
    }
}

impl Drop for Conn {
    /// Останавливает задачу соединения: подписки закрываются вместе с ним.
    fn drop(&mut self) {
        self.task.abort();
    }
}
