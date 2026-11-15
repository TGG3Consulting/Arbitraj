//! Файл: venues/bsc_dex/node/head_stall.rs
//! Что делает: решение 282 — уведомление «узел BSC: блок не растёт». Инцидент 14.11.2026: свой узел молча встал
//! (процесс жив, RPC отвечает, eth_blockNumber не растёт) — бот перестал торговать (StaleData), но в Telegram ничего
//! не ушло: о зависании были только записи журнала (venue.head_age, venue.heads).
//! Номер блока берётся из уже идущего опроса eth_blockNumber (решение 57, node/heads.rs) — своего опроса нет.
//! Номер не менялся дольше bsc.node_stall_alert_s (отсчёт — от момента, когда бот увидел этот номер: Head.received_at)
//! — ОДНО событие dex.node_stalled; номер вырос — ОДНО событие dex.node_resumed с длительностью простоя. Повторов нет.
//! Состояние «уже сообщено» живёт в Heads (а не в задаче опроса): пересборка подписки перезапускает задачу опроса, но
//! второго сообщения о том же простое не будет. Настройка не задана — ничего не отправляется и не пишется (как раньше).
//! Каждое сообщение пишется и в журнал (вид venue.head_stall) — выключенное событие второй записи не делает
//! (notify/journal_off.rs OWN_RECORD).
//! Кто вызывает: node/heads.rs (run — каждый круг опроса, и при сбое опроса тоже); parts.rs (set_out — канал ядру).
//! От чего зависит: ctx.rs, node/heads.rs (Head), node/head_stall_text.rs (текст), gateway/events.rs
//! (MarketEvent::Notice), storage/journal.
//! Настройки: bsc.node_stall_alert_s (вся система, необязательная); включение событий — notify.event.* с областью
//! события (экран «Уведомления»).

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::head_stall_text::text;
use super::heads::Head;

/// Порог простоя номера блока, секунды.
pub const SETTING: &str = "bsc.node_stall_alert_s";
/// Событие уведомлений: номер блока узла не растёт дольше порога.
pub const CODE_STALLED: &str = "dex.node_stalled";
/// Событие уведомлений: номер блока снова растёт.
pub const CODE_RESUMED: &str = "dex.node_resumed";

/// Что сообщить.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Номер стоит: голова, на которой стоит, и сколько мс.
    Stalled { head: Head, stood_ms: i64 },
    /// Номер вырос: на чём стоял, новая голова, простой в мс (от получения прежней до получения новой).
    Resumed { was: Head, now: Head, idle_ms: i64 },
}

/// Состояние уведомления. Дешёво клонируется (живёт в Heads).
#[derive(Clone, Default)]
pub struct Stall {
    /// Голова, о простое на которой уже сообщено; None — не сообщалось.
    told: Arc<Mutex<Option<Head>>>,
    /// Канал событий ядру (задаётся при подписке на рынок).
    out: Arc<Mutex<Option<mpsc::Sender<MarketEvent>>>>,
}

/// Переход (чистая, тестируется). Принимает: голову, о которой сообщено; последнюю голову; порог в секундах (None — не
/// задан); сейчас. Возвращает: новое «сообщено» и что отправить. Ошибок нет.
pub fn decide(
    told: Option<&Head>,
    latest: Option<&Head>,
    limit_s: Option<u64>,
    now: DateTime<Utc>,
) -> (Option<Head>, Option<Change>) {
    // Порог не задан — функции нет: состояние сбрасывается без сообщений.
    let Some(limit_s) = limit_s else {
        return (None, None);
    };
    let Some(latest) = latest else {
        return (told.cloned(), None);
    };
    if let Some(was) = told {
        if latest.number > was.number {
            let idle_ms = (latest.received_at - was.received_at).num_milliseconds();
            let c = Change::Resumed {
                was: was.clone(),
                now: latest.clone(),
                idle_ms,
            };
            return (None, Some(c));
        }
        return (Some(was.clone()), None);
    }
    let stood_ms = (now - latest.received_at).num_milliseconds();
    let limit_ms = i64::try_from(limit_s.saturating_mul(1000)).unwrap_or(i64::MAX);
    if stood_ms > limit_ms {
        let c = Change::Stalled {
            head: latest.clone(),
            stood_ms,
        };
        return (Some(latest.clone()), Some(c));
    }
    (None, None)
}

impl Stall {
    /// Канал событий ядру (parts.rs при подписке на рынок). Принимает: канал. Ошибок нет.
    pub fn set_out(&self, out: mpsc::Sender<MarketEvent>) {
        *self.out.lock().unwrap_or_else(|p| p.into_inner()) = Some(out);
    }

    /// Один круг опроса: переход и, если есть, сообщение в журнал и ядру (Telegram по коду события).
    /// Принимает: окружение, последнюю голову, время блока (Heads::block_time_ms), последнюю ошибку опроса, ошибку
    /// проверки свежести. Ошибок не возвращает.
    pub async fn step(
        &self,
        ctx: &Ctx,
        latest: Option<Head>,
        block_time: Option<(u64, u64)>,
        poll_err: Option<&str>,
        fresh_err: Option<&str>,
    ) {
        let limit = ctx.settings.integer(SETTING, At::default()).ok();
        let change = {
            let mut told = self.told.lock().unwrap_or_else(|p| p.into_inner());
            let (next, change) = decide(told.as_ref(), latest.as_ref(), limit, Utc::now());
            *told = next;
            change
        };
        let (Some(c), Some(limit)) = (change, limit) else {
            return;
        };
        let msg = text(&c, limit, block_time, poll_err, fresh_err);
        let (level, code, head) = match c {
            Change::Stalled { .. } => (Level::Warn, CODE_STALLED, "Узел BSC: блок не растёт"),
            Change::Resumed { .. } => (Level::Info, CODE_RESUMED, "Узел BSC: блоки снова идут"),
        };
        let journal = format!("{head}\n{msg}");
        ctx.log(level, "venue.head_stall", journal, None).await;
        self.send(ctx, code, msg).await;
    }

    /// Событие ядру без молчаливой потери (как node/receipts_notice.rs): канал полон — отдельной задачей; канала нет
    /// или закрыт — запись в журнал, что событие не ушло. Принимает: окружение, код, текст. Ошибок нет.
    async fn send(&self, ctx: &Ctx, code: &str, text: String) {
        use tokio::sync::mpsc::error::TrySendError;
        let out = self.out.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let ev = MarketEvent::Notice {
            venue: ctx.venue(),
            code: code.into(),
            text,
        };
        let lost = match out {
            None => Some("канал событий ядру ещё не открыт"),
            Some(out) => match out.try_send(ev) {
                Ok(()) => None,
                Err(TrySendError::Full(ev)) => {
                    let (ctx, code) = (ctx.clone(), code.to_string());
                    tokio::spawn(async move {
                        if out.send(ev).await.is_err() {
                            let msg =
                                format!("Уведомление {code} не ушло — канал событий ядру закрыт");
                            ctx.log(Level::Warn, "venue.head_stall", msg, None).await;
                        }
                    });
                    None
                }
                Err(TrySendError::Closed(_)) => Some("канал событий ядру закрыт"),
            },
        };
        if let Some(why) = lost {
            let msg = format!("Уведомление {code} не ушло — {why}");
            ctx.log(Level::Warn, "venue.head_stall", msg, None).await;
        }
    }
}
