//! Файл: venues/bsc_dex/directory/limit.rs
//! Что делает: темп запросов к индексатору — строго по настройке index.requests_per_min: каждый запрос занимает
//! 60 / N секунд в своей очереди (отдельной от очереди узла BSC). Ответ «превышен лимит» (HTTP 429,
//! ErrorCode::RateLimited) — по правилу шлюза «Превышен лимит запросов» площадки bsc_dex или общему (как у узла,
//! node/pace.rs): Retry { attempts, pause_ms } — очередь ждёт pause_ms, запрос повторяется; иначе — ошибка.
//! Кто вызывает: directory/gt_http.rs.
//! От чего зависит: ctx.rs, node/pace.rs (Pace, retry_rule).
//! Настройки: index.requests_per_min [bsc_dex]; правила шлюза для RateLimited.
//!
//! Лимиты CoinGecko (в минуту, ошибки тоже считаются, превышение — 429) —
//! https://docs.coingecko.com/docs/errors-and-rate-limits ; без ключа GeckoTerminal «~10 calls/min» —
//! https://docs.coingecko.com/docs/keyless-public-api . Число задаёт Тигран под свой тариф.

use std::time::Duration;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::pace;

/// Шаг очереди: 60 с / index.requests_per_min. Ошибки: NotConfigured.
pub fn step(ctx: &Ctx) -> GatewayResult<Duration> {
    let per_min = ctx.int("index.requests_per_min")?;
    Ok(Duration::from_secs_f64(60.0 / per_min.max(1) as f64))
}

/// Запрос к индексатору в очереди и с правилом шлюза на RateLimited. Принимает: сам запрос.
/// Возвращает: его ответ. Ошибки: как у запроса; NotConfigured (темп).
pub async fn run<T, F, Fut>(ctx: &Ctx, send: F) -> GatewayResult<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = GatewayResult<T>>,
{
    let mut tries = 0u32;
    loop {
        let at = ctx.index.pace.reserve(step(ctx)?);
        tokio::time::sleep_until(at).await;
        let err = match send().await {
            Err(e) if e.code == ErrorCode::RateLimited => e,
            other => return other,
        };
        match pace::retry_rule(ctx).await {
            Some((attempts, pause_ms)) if tries < attempts => {
                tries += 1;
                ctx.index.pace.hold(Duration::from_millis(pause_ms));
            }
            _ => return Err(err),
        }
    }
}
