//! Файл: venues/bsc_dex/node/pace.rs
//! Что делает: темп запросов к узлу BSC по HTTP. Каждый запрос занимает своё время в общей очереди: не чаще
//! bsc.node_http_rps вызовов в секунду (пачка JSON-RPC из N вызовов — N мест: провайдер считает каждый вызов).
//! Так первый проход проверок (сотни монет сразу) идёт ровно, а не залпом. Ответ «превышен лимит» (HTTP 429,
//! ErrorCode::RateLimited) — по правилу шлюза venue.rate_limited (gateway/rules.rs: сначала правило площадки
//! bsc_dex, потом общее): Retry { attempts, pause_ms } — вся очередь узла ждёт pause_ms, запрос повторяется, не
//! больше attempts раз; другого правила или его нет — ошибка уходит вызывающему (проверка «не выполнена», не вердикт).
//! Кто вызывает: node/rpc.rs (call_raw, batch); очередь и правило — ещё directory/limit.rs (индексатор).
//! От чего зависит: ctx.rs, gateway/rules.rs, storage/rules.rs (правила из SQLite журнала ctx).
//! Настройки: bsc.node_http_rps (вся система); правила шлюза для RateLimited.
//!
//! HTTP 429 Too Many Requests — RFC 6585, раздел 4 (https://www.rfc-editor.org/rfc/rfc6585#section-4).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::rules::{RuleAction, pick_rule};
use crate::settings::read::At;

use super::super::ctx::Ctx;

/// Общая очередь запросов к узлу HTTP. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Pace {
    /// Когда может уйти следующий запрос. None — сразу.
    next: Arc<Mutex<Option<Instant>>>,
}

impl Pace {
    /// Занимает место в очереди на время `step`. Возвращает: когда отправлять. Ошибок нет.
    /// Его же использует очередь индексатора (directory/limit.rs).
    pub(crate) fn reserve(&self, step: Duration) -> Instant {
        let mut next = self.next.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        let slot = next.map_or(now, |n| n.max(now));
        *next = Some(slot + step);
        slot
    }

    /// Вся очередь ждёт не меньше `pause` от текущего момента. Ошибок нет.
    pub(crate) fn hold(&self, pause: Duration) {
        let mut next = self.next.lock().unwrap_or_else(|p| p.into_inner());
        let until = Instant::now() + pause;
        *next = Some(next.map_or(until, |n| n.max(until)));
    }
}

/// Ждёт своей очереди на `weight` вызовов. Ошибки: NotConfigured (bsc.node_http_rps не задана) — запрос не уходит.
pub async fn wait(ctx: &Ctx, weight: usize) -> GatewayResult<()> {
    let rps = ctx
        .settings
        .integer("bsc.node_http_rps", At::default())
        .map_err(|e| ctx.tag(e))?;
    let step = Duration::from_secs_f64(weight.max(1) as f64 / rps as f64);
    tokio::time::sleep_until(ctx.pace.reserve(step)).await;
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
