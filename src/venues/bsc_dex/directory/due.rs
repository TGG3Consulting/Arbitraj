//! Файл: venues/bsc_dex/directory/due.rs
//! Что делает: сроки обновления справочника индексатора: через сколько подойдёт ближайшая монета или список DEX
//! (у монеты с ошибкой — свой срок, index.error_retry_s) и какие ошибки останавливают весь проход.
//! Кто вызывает: directory/refresh.rs, directory/fetch.rs.
//! От чего зависит: directory/mod.rs (TokenEntry, Snap), ctx.rs.
//! Настройки: нет своих (сроки приходят от refresh.rs).

use std::time::Duration;

use chrono::Utc;

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::super::ctx::Ctx;

/// Ошибка, при которой проход не продолжается (настройка, секреты): остальные монеты дадут то же. Ошибок нет.
pub fn stops(e: &GatewayError) -> bool {
    matches!(e.code, ErrorCode::NotConfigured | ErrorCode::SecretsLocked)
}

/// Через сколько подойдёт срок ближайшей монеты или списка DEX (не меньше нуля). Ошибок нет.
pub fn next_due(
    ctx: &Ctx,
    every: chrono::Duration,
    gap: &dyn Fn(&super::TokenEntry) -> chrono::Duration,
) -> Duration {
    let now = Utc::now();
    let soonest = ctx.index.with(|s| {
        s.tokens
            .values()
            .map(|t| t.tried_at.or(t.fetched_at).map_or(now, |at| at + gap(t)))
            .chain(std::iter::once(
                s.dexes_tried.or(s.dexes_at).map_or(now, |at| at + every),
            ))
            .min()
    });
    soonest
        .and_then(|t| (t - now).to_std().ok())
        .unwrap_or(Duration::ZERO)
}
