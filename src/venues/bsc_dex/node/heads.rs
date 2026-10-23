//! Файл: venues/bsc_dex/node/heads.rs
//! Что делает: постоянная подписка newHeads — последний блок сети, его свежесть («не торгуем вслепую»)
//! и обнаружение реорга (новая голова не продолжает прежнюю). Обрыв — переподключение после паузы.
//! Кто вызывает: mod.rs (запуск при подписке на рынок), quote/*, exec/*, send/track.rs (номер блока, свежесть).
//! От чего зависит: ctx.rs, node/ws.rs, errors.rs.
//! Настройки: feed.stale_after_ms [bsc_dex] (голова старше — данные устарели), venue.ws_reconnect_pause_ms [bsc_dex];
//! узел — bsc.node_ws_url.
//!
//! Источники: eth_subscribe "newHeads" —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml#L1-L60 ;
//! при реорге «will emit the last header in the new chain» — https://geth.ethereum.org/docs/interacting-with-geth/rpc/pubsub ;
//! поля заголовка (number, hash, parentHash, timestamp — hex-числа) — execution-apis, schemas/block.yaml.

use std::time::Duration;

use alloy_primitives::{B256, U64};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::watch;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::ws::Conn;

/// Голова сети, как её прислал узел.
#[derive(Debug, Clone, PartialEq)]
pub struct Head {
    pub number: u64,
    pub hash: B256,
    pub parent: B256,
    /// Время блока (секунды Unix) — по нему считаются deadline обменов.
    pub timestamp: u64,
    /// Когда получено на нашем сервере.
    pub received_at: DateTime<Utc>,
}

/// Заголовок из уведомления newHeads: только нужные поля.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHead {
    number: U64,
    hash: B256,
    parent_hash: B256,
    timestamp: U64,
}

/// Последняя голова; дешёво клонируется, слушателей сколько угодно.
#[derive(Clone)]
pub struct Heads {
    tx: watch::Sender<Option<Head>>,
}

impl Default for Heads {
    fn default() -> Self {
        Self {
            tx: watch::channel(None).0,
        }
    }
}

impl Heads {
    /// Приёмник новых голов (для трекера транзакций и пулов). Ошибок нет.
    pub fn watch(&self) -> watch::Receiver<Option<Head>> {
        self.tx.subscribe()
    }

    /// Последняя голова без проверки свежести. None — ещё не было. Ошибок нет.
    pub fn latest(&self) -> Option<Head> {
        self.tx.borrow().clone()
    }

    /// Свежая голова: получена не раньше feed.stale_after_ms назад.
    /// Ошибки: StaleData (нет головы или устарела), NotConfigured (порог не задан).
    pub fn fresh(&self, ctx: &Ctx) -> GatewayResult<Head> {
        let limit = ctx.int("feed.stale_after_ms")? as i64;
        let head = self.latest().ok_or_else(|| {
            ctx.err(
                ErrorCode::StaleData,
                "Нет данных о блоках сети BSC (newHeads)",
            )
        })?;
        let age = (Utc::now() - head.received_at).num_milliseconds();
        if age > limit {
            return Err(ctx.err(
                ErrorCode::StaleData,
                format!("Последний блок {} получен {age} мс назад — больше feed.stale_after_ms ({limit})", head.number),
            ));
        }
        Ok(head)
    }

    /// Работает постоянно: подписка newHeads, при обрыве — пауза venue.ws_reconnect_pause_ms и снова.
    /// Не задан узел или пауза — запись в журнал и выход (запустится снова при следующей подписке на рынок).
    /// Принимает: окружение. Ошибок не возвращает.
    pub async fn run(self, ctx: Ctx) {
        loop {
            if let Err(e) = self.session(&ctx).await {
                ctx.log(
                    Level::Error,
                    "venue.ws",
                    format!("newHeads: {}", e.message),
                    None,
                )
                .await;
            }
            let pause = match ctx.int("venue.ws_reconnect_pause_ms") {
                Ok(ms) => Duration::from_millis(ms),
                Err(e) => {
                    ctx.log(
                        Level::Error,
                        "venue.ws",
                        format!("newHeads остановлены: {}", e.message),
                        None,
                    )
                    .await;
                    return;
                }
            };
            tokio::time::sleep(pause).await;
        }
    }

    /// Одна сессия подписки: до обрыва соединения.
    /// Ошибки: подключение, отказ подписки, непонятный заголовок.
    async fn session(&self, ctx: &Ctx) -> GatewayResult<()> {
        let conn = Conn::open(ctx, "heads").await?;
        let mut sub = conn.subscribe(json!(["newHeads"])).await?;
        while let Some(raw) = sub.rx.recv().await {
            let h: RawHead = serde_json::from_str(raw.get()).map_err(|e| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("newHeads: непонятный заголовок: {e}"),
                )
                .with_raw(raw.get())
            })?;
            let head = Head {
                number: h.number.to(),
                hash: h.hash,
                parent: h.parent_hash,
                timestamp: h.timestamp.to(),
                received_at: Utc::now(),
            };
            if let Some(prev) = self.latest()
                && head.parent != prev.hash
                && head.hash != prev.hash
            {
                let msg = format!(
                    "Реорг сети BSC: блок {} ({}) не продолжает {} ({})",
                    head.number, head.hash, prev.number, prev.hash
                );
                ctx.log(Level::Warn, "dex.reorg", msg, None).await;
            }
            self.tx.send_replace(Some(head));
        }
        Err(ctx.err(
            ErrorCode::Unreachable,
            conn.end_reason()
                .unwrap_or_else(|| "подписка закрыта".into()),
        ))
    }
}
