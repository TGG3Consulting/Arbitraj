//! Файл: venues/bsc_dex/node/pace.rs
//! Что делает: темп запросов к узлу BSC по HTTP. Каждый запрос занимает своё время в общей очереди: не чаще
//! bsc.node_http_rps вызовов в секунду (пачка JSON-RPC из N вызовов — N мест: провайдер считает каждый вызов).
//! Так первый проход проверок (сотни монет сразу) идёт ровно, а не залпом. Ответ «превышен лимит» (HTTP 429,
//! ErrorCode::RateLimited) — по правилу шлюза venue.rate_limited (gateway/rules.rs: сначала правило площадки
//! bsc_dex, потом общее): Retry { attempts, pause_ms } — вся очередь узла ждёт pause_ms, запрос повторяется, не
//! больше attempts раз; другого правила или его нет — ошибка уходит вызывающему (проверка «не выполнена», не вердикт).
//! Срочность (решение 61): освободившееся место получает первый ждущий старшего класса — сделка, потом проверки
//! монет, потом фон (node/priority.rs); внутри класса — по приходу. Отменённый запрос уходит из очереди.
//! Кто вызывает: node/rpc.rs (call_raw, batch); очередь (без классов) и правило — ещё directory/limit.rs (индексатор).
//! От чего зависит: ctx.rs, node/priority.rs, gateway/rules.rs, storage/rules.rs (правила из SQLite журнала ctx).
//! Настройки: bsc.node_http_rps (вся система); правила шлюза для RateLimited.
//!
//! HTTP 429 Too Many Requests — RFC 6585, раздел 4 (https://www.rfc-editor.org/rfc/rfc6585#section-4).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::rules::{RuleAction, pick_rule};
use crate::settings::read::At;

use super::super::ctx::Ctx;
use super::priority::{self, Class};

/// Общая очередь запросов к узлу HTTP. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Pace {
    state: Arc<Mutex<State>>,
    /// Будит ждущих: место отдано, ждущий ушёл, пауза очереди изменилась.
    wake: Arc<Notify>,
}

#[derive(Default)]
struct State {
    /// Когда может уйти следующий запрос. None — сразу.
    next: Option<Instant>,
    /// Ждущие места: (класс, номер прихода). Первый — следующий на отправку.
    waiting: BTreeSet<(Class, u64)>,
    /// Номер следующего пришедшего.
    seq: u64,
}

/// Место ждущего в очереди; уходит из очереди при отмене запроса (drop) — чтобы не задерживать других.
struct Ticket<'a> {
    pace: &'a Pace,
    key: (Class, u64),
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        if self.pace.lock().waiting.remove(&self.key) {
            self.pace.wake.notify_waiters();
        }
    }
}

impl Pace {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Занимает место в очереди на время `step` без классов (одна очередь индексатора, directory/limit.rs).
    /// Возвращает: когда отправлять. Ошибок нет.
    pub(crate) fn reserve(&self, step: Duration) -> Instant {
        let mut st = self.lock();
        let now = Instant::now();
        let slot = st.next.map_or(now, |n| n.max(now));
        st.next = Some(slot + step);
        slot
    }

    /// Вся очередь ждёт не меньше `pause` от текущего момента. Ошибок нет.
    pub(crate) fn hold(&self, pause: Duration) {
        let mut st = self.lock();
        let until = Instant::now() + pause;
        st.next = Some(st.next.map_or(until, |n| n.max(until)));
        drop(st);
        self.wake.notify_waiters();
    }

    /// Ждёт своего места с классом `class` и занимает его на `step`: место отдаётся первому из старшего класса,
    /// внутри класса — по приходу; отправка — не раньше, чем освободится очередь. Ошибок нет.
    pub async fn take(&self, class: Class, step: Duration) {
        let ticket = {
            let mut st = self.lock();
            st.seq += 1;
            let key = (class, st.seq);
            st.waiting.insert(key);
            Ticket { pace: self, key }
        };
        loop {
            // Подписка до проверки: пробуждение между проверкой и ожиданием не теряется (Notify::notified).
            let woke = self.wake.notified();
            let wait_until = {
                let mut st = self.lock();
                let now = Instant::now();
                if st.waiting.first() != Some(&ticket.key) {
                    None
                } else if st.next.is_none_or(|n| n <= now) {
                    st.waiting.remove(&ticket.key);
                    st.next = Some(now + step);
                    drop(st);
                    self.wake.notify_waiters();
                    return;
                } else {
                    st.next
                }
            };
            match wait_until {
                Some(at) => {
                    tokio::select! {
                        _ = tokio::time::sleep_until(at) => {}
                        _ = woke => {}
                    }
                }
                None => woke.await,
            }
        }
    }
}

/// Ждёт своей очереди на `weight` вызовов с классом текущей задачи (node/priority.rs).
/// Ошибки: NotConfigured (bsc.node_http_rps не задана) — запрос не уходит.
pub async fn wait(ctx: &Ctx, weight: usize) -> GatewayResult<()> {
    let rps = ctx
        .settings
        .integer("bsc.node_http_rps", At::default())
        .map_err(|e| ctx.tag(e))?;
    if rps == 0 {
        let m = "bsc.node_http_rps должна быть больше нуля";
        return Err(ctx.err(ErrorCode::NotConfigured, m));
    }
    let step = Duration::from_secs_f64(weight.max(1) as f64 / rps as f64);
    ctx.pace.take(priority::current(), step).await;
    Ok(())
}

/// Запрос к узлу с темпом и правилом шлюза на RateLimited. Принимает: число вызовов в запросе, сам запрос.
/// Возвращает: его ответ. Ошибки: как у запроса; NotConfigured (темп); сбой чтения правил — исходная ошибка.
pub async fn run<T, F, Fut>(ctx: &Ctx, weight: usize, send: F) -> GatewayResult<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = GatewayResult<T>>,
{
    let mut tries = 0u32;
    loop {
        wait(ctx, weight).await?;
        let err = match send().await {
            Err(e) if e.code == ErrorCode::RateLimited => e,
            other => return other,
        };
        match retry_rule(ctx).await {
            Some((attempts, pause_ms)) if tries < attempts => {
                tries += 1;
                ctx.pace.hold(Duration::from_millis(pause_ms));
            }
            _ => return Err(err),
        }
    }
}

/// Правило шлюза на RateLimited для bsc_dex: (повторов, пауза мс), если это Retry. None — правила нет, оно другое
/// или правила не прочитались. Ошибок нет.
pub(crate) async fn retry_rule(ctx: &Ctx) -> Option<(u32, u64)> {
    let rules = crate::storage::rules::list(ctx.journal.db()).await.ok()?;
    match pick_rule(&rules, ErrorCode::RateLimited, Some(&ctx.venue()))?.action {
        RuleAction::Retry { attempts, pause_ms } => Some((attempts, pause_ms)),
        _ => None,
    }
}
