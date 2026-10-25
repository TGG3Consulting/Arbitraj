//! Файл: venues/bsc_dex/node/pace.rs
//! Что делает: темп запросов к узлу BSC по HTTP. Каждый запрос занимает своё время в общей очереди: не чаще
//! bsc.node_http_rps вызовов в секунду (пачка JSON-RPC из N вызовов — N мест: провайдер считает каждый вызов).
//! Так первый проход проверок (сотни монет сразу) идёт ровно, а не залпом. Ответ «превышен лимит» (HTTP 429,
//! ErrorCode::RateLimited) — по правилу шлюза venue.rate_limited (gateway/rules.rs: сначала правило площадки
//! bsc_dex, потом общее): Retry { attempts, pause_ms } — вся очередь узла ждёт pause_ms, запрос повторяется, не
//! больше attempts раз; другого правила или его нет — ошибка уходит вызывающему (проверка «не выполнена», не вердикт).
//! Срочность (решение 61): очередь со строгим порядком классов и гарантированной долей фона — node/queue.rs,
//! классы — gateway/urgency.rs.
//! Кто вызывает: node/rpc.rs (call_raw, batch); правило — ещё directory/limit.rs (индексатор).
//! От чего зависит: ctx.rs, node/queue.rs, gateway/urgency.rs, gateway/rules.rs, storage/rules.rs.
//! Настройки: bsc.node_http_rps, bsc.node_background_share_pct (вся система; не задана — строгий порядок без доли
//! фона); правила шлюза для RateLimited.
//!
//! HTTP 429 Too Many Requests — RFC 6585, раздел 4 (https://www.rfc-editor.org/rfc/rfc6585#section-4).

use std::time::Duration;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::rules::{RuleAction, pick_rule};
use crate::gateway::urgency;
use crate::settings::read::At;

use super::super::ctx::Ctx;
pub use super::queue::Pace;

/// Ждёт своей очереди на `weight` вызовов с классом текущей задачи (gateway/urgency.rs) и долей фона
/// bsc.node_background_share_pct (не задана — строгий порядок, node/queue.rs).
/// Ошибки: NotConfigured (bsc.node_http_rps не задана) — запрос не уходит; Internal — доля не читается.
pub async fn wait(ctx: &Ctx, weight: usize) -> GatewayResult<()> {
    let rps = ctx
        .settings
        .integer("bsc.node_http_rps", At::default())
        .map_err(|e| ctx.tag(e))?;
    if rps == 0 {
        let m = "bsc.node_http_rps должна быть больше нуля";
        return Err(ctx.not_set("bsc.node_http_rps", m));
    }
    let step = Duration::from_secs_f64(weight.max(1) as f64 / rps as f64);
    let share = match ctx
        .settings
        .decimal("bsc.node_background_share_pct", At::default())
        .map_err(|e| ctx.tag(e))
    {
        Ok(p) => rust_decimal::prelude::ToPrimitive::to_f64(&p).unwrap_or(0.0),
        Err(e) if e.code == ErrorCode::NotConfigured => 0.0,
        Err(e) => return Err(e),
    };
    ctx.pace.take(urgency::current(), step, share).await;
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
