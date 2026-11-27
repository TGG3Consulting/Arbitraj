//! Общий скользящий бюджет POST и GET Ryze Relayer для всех кошельков одной базы бота.
//! Router /quote использует другой сервис. Окно и Retry-After переживают перезапуск.
use crate::{storage::db::Db, venues::bsc_dex::ctx::Ctx};
use super::budget_window::{self as window, Action, Key};

// Опубликованный потолок на IP за минуту: https://documentation.ryze.pro/relayer/overview
const IP_MAX: u64 = 100;
pub(super) fn limit(ctx: &Ctx) -> Option<u64> {
    ctx.int("ryze.relayer_requests_per_minute")
        .ok()
        .filter(|v| *v > 0)
        .map(|v| v.min(IP_MAX))
}

pub(super) const REJECTED: &str =
    "Ryze Relayer: HTTP 429, релей отклонил отправку по лимиту запросов";

/// Проверка при выборе не расходует окно. Первичный POST и GET статуса пользуются общей записью.
pub(super) async fn available(ctx: &Ctx) -> Result<(), String> {
    let max = limit(ctx).ok_or("Ryze Relayer: не задан ryze.relayer_requests_per_minute")?;
    window::available(ctx.journal.db(), Key::Relayer, max).await
}

/// Резервирует один реальный HTTP-запрос. Если окно занято, вызов немедленно получает причину отказа.
pub(super) async fn take(ctx: &Ctx) -> Result<(), String> {
    let max = limit(ctx).ok_or("Ryze Relayer: не задан ryze.relayer_requests_per_minute")?;
    window::update(ctx.journal.db(), Key::Relayer, Action::Request(max)).await
}
pub(super) async fn pause(
    db: &Db,
    seconds: Option<u64>,
    rejected: Option<String>,
) -> Result<(), String> {
    window::update(db, Key::Relayer, Action::Pause(seconds, rejected)).await
}

/// Только код ответа на исходный POST означает, что подписанное поручение было отклонено.
pub(super) async fn submit_rejected(db: &Db, claim: String) -> Result<bool, String> {
    db.ledger()
        .read(move |c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM core_state WHERE kind='ryze_submit_429' AND id=?1)",
                [claim],
                |r| r.get(0),
            )
        })
        .await
        .map_err(|e| e.to_string())
}


/// Для Router владелец задаёт самостоятельный предел. Origin объединяет пути и кошельки одного сервера.
fn router_limit(ctx: &Ctx) -> Option<u64> {
    if ctx.chain_id() != Some(super::CHAIN_ID) {
        return None;
    }
    ctx.int("ryze.router_requests_per_minute").ok().filter(|n| *n > 0)
}

pub(super) fn router_ready(ctx: &Ctx, origin: &str) -> bool {
    router_limit(ctx).is_some_and(|max| window::router_ready(origin, max))
}

pub(super) async fn take_router(ctx: &Ctx, url: &reqwest::Url) -> Option<()> {
    let max = router_limit(ctx)?;
    let origin = url.origin().ascii_serialization();
    if !window::router_ready(&origin, max) {
        return None;
    }
    window::update(ctx.journal.db(), Key::Router(origin.clone()), Action::Request(max)).await.ok()?;
    // Другая задача могла получить 429, пока наша заявка ждала SQLite. Последнее место окна уже наше.
    window::router_unpaused(&origin).then_some(())
}

pub(super) async fn pause_router(ctx: &Ctx, url: &reqwest::Url, seconds: Option<u64>) -> Result<(), String> {
    let origin = url.origin().ascii_serialization();
    window::block_router(&origin, seconds);
    window::update(ctx.journal.db(), Key::Router(origin), Action::Pause(seconds, None)).await
}
