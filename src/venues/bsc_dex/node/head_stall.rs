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
//! Задача t122 (п. 5): круг опроса читает порог ДО проверки свежести (limit → heads.rs: fresh() — только при заданном
//! пороге); «сообщено» ставится ПОСЛЕ того, как событие ушло в канал ядру (или встало в очередь отдельной задачей):
//! канала нет или он закрыт — «сообщено» не меняется, следующий круг молча пробует отправить снова; запись журнала с
//! текстом и запись «не ушло» — по одному разу на простой / восстановление (ревью t122: флаг State.said). Состояние
//! под асинхронным замком на всё время отправки — две задачи опроса (старая живёт до удачной пересборки подписки) не
//! шлют одно и то же дважды.
//! Кто вызывает: node/heads.rs (run — каждый круг опроса, и при сбое опроса тоже: limit, step_with); parts.rs (set_out
//! — канал ядру); tests/bsc_dex_head_stall.rs (step).
//! От чего зависит: ctx.rs, node/heads.rs (Head), node/head_stall_text.rs (текст), node/head_stall_send.rs (отправка), gateway/events.rs
//! (MarketEvent::Notice), storage/journal.
//! Настройки: bsc.node_stall_alert_s (вся система + площадка DEX — решение 435, необязательная); включение событий — notify.event.* с областью
//! события (экран «Уведомления»).

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;
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
    /// «Сообщено» и «записано в журнал». Асинхронный замок — держится на время журнала и отправки (задача t122, п. 5).
    state: Arc<tokio::sync::Mutex<State>>,
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

/// Состояние под замком Stall.
#[derive(Default)]
struct State {
    /// Голова, о простое на которой уже сообщено (событие ушло); None — не сообщалось.
    told: Option<Head>,
    /// Ревью t122: (код события, номер блока) перехода, уже записанного в журнал, но ещё не ушедшего ядру — повторная
    /// попытка отправки журнал не пишет. Сбрасывается, когда событие ушло или переход пропал.
    said: Option<(&'static str, u64)>,
}

impl Stall {
    /// Канал событий ядру (parts.rs при подписке на рынок). Принимает: канал. Ошибок нет.
    pub fn set_out(&self, out: mpsc::Sender<MarketEvent>) {
        *self.out.lock().unwrap_or_else(|p| p.into_inner()) = Some(out);
    }

    /// Порог простоя из настройки (задача t122, п. 5: круг опроса читает его ДО fresh()). Принимает: окружение.
    /// Возвращает: секунды; None — не задан (или не читается) — функции нет. Ошибок нет.
    pub fn limit(ctx: &Ctx) -> Option<u64> {
        ctx.settings.integer(SETTING, ctx.at()).ok()
    }

    /// Один круг опроса с порогом из настройки (limit, затем step_with). Принимает и ошибок — как step_with.
    pub async fn step(
        &self,
        ctx: &Ctx,
        latest: Option<Head>,
        block_time: Option<(u64, u64)>,
        poll_err: Option<&str>,
        fresh_err: Option<&str>,
    ) {
        let limit = Self::limit(ctx);
        self.step_with(ctx, limit, latest, block_time, poll_err, fresh_err)
            .await;
    }

    /// Один круг опроса: переход и, если есть, сообщение в журнал и ядру (Telegram по коду события). «Сообщено»
    /// меняется сразу, если сообщать нечего, и только после ушедшего события, если есть что (задача t122, п. 5).
    /// Принимает: окружение, порог (Stall::limit), последнюю голову, время блока (Heads::block_time_ms), последнюю
    /// ошибку опроса, ошибку проверки свежести. Ошибок не возвращает.
    pub async fn step_with(
        &self,
        ctx: &Ctx,
        limit: Option<u64>,
        latest: Option<Head>,
        block_time: Option<(u64, u64)>,
        poll_err: Option<&str>,
        fresh_err: Option<&str>,
    ) {
        let mut st = self.state.lock().await;
        let (next, change) = decide(st.told.as_ref(), latest.as_ref(), limit, Utc::now());
        let (Some(c), Some(limit)) = (change, limit) else {
            (st.told, st.said) = (next, None);
            return;
        };
        let msg = text(&c, limit, block_time, poll_err, fresh_err);
        let (level, code, head, n) = match &c {
            Change::Stalled { head, .. } => (
                Level::Warn,
                CODE_STALLED,
                "Узел BSC: блок не растёт",
                head.number,
            ),
            Change::Resumed { was, .. } => (
                Level::Info,
                CODE_RESUMED,
                "Узел BSC: блоки снова идут",
                was.number,
            ),
        };
        // Ревью t122: журнал — один раз на переход; повторная попытка после «не ушло» — молча.
        let first = st.said != Some((code, n));
        if first {
            let journal = format!("{head}\n{msg}");
            ctx.log(level, "venue.head_stall", journal, None).await;
        }
        let out = self.out.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if super::head_stall_send::send(out, ctx, code, msg, first).await {
            (st.told, st.said) = (next, None);
        } else {
            st.said = Some((code, n));
        }
    }
}
