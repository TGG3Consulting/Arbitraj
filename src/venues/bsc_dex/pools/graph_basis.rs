//! Файл: venues/bsc_dex/pools/graph_basis.rs
//! Что делает: решение 308 (задача t137) — на чём строится снимок графа сейчас (basis: настройки графа, USDT и его
//! decimals) и dex.route_max_pools_per_edge; вынесено из graph_step.rs (размер файла).
//! - Config — не задана/неверна настройка графа или глубины, USDT не известен: функция не работает, снимка нет;
//! - Transient — временный сбой чтения decimals USDT у узла: если прежний basis (карты или готовой сборки) совпадает
//!   с текущими настройками графа и USDT — берётся он (ревью №2 F, повторное ревью ж): снимок, карта и готовая сборка
//!   не выбрасываются; иначе — повтор на следующем пробуждении.
//!
//! Кто вызывает: graph_step.rs, graph_finish.rs.
//! От чего зависит: graph_owner.rs (Owner), graph_snap.rs (Basis), graph_cache.rs (graph_settings), depth_book.rs,
//! tokens.rs, ctx.rs.
//! Настройки: dex.route_max_pools_per_edge [bsc_dex]; через книгу глубины — dex.depth_band_pct, dex.min_pool_liquidity_usd,
//! dex.depth_hysteresis_pct; venue.quote_asset (USDT).

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::depth_book::{self, Book};
use super::graph_cache::graph_settings;
use super::graph_owner::Owner;
use super::graph_snap::Basis;

/// Почему basis не получен.
pub(super) enum NoBasis {
    /// Не задана / неверна настройка, USDT не известен — функция не работает: снимка нет.
    Config(String),
    /// Временный сбой (decimals USDT у узла), прежнего подходящего basis нет — повтор.
    Transient(String),
}

/// Basis сейчас (см. шапку). Принимает: окружение владельца, прежний basis (карты или сборки) на случай временного сбоя.
/// Возвращает: basis, dex.route_max_pools_per_edge и текст временного сбоя (если basis взят прежний).
/// Ошибки: Config, Transient.
pub(super) async fn basis_or(
    o: &Owner,
    prev: Option<&Basis>,
) -> Result<(Basis, usize, Option<String>), NoBasis> {
    let per_edge = checks(&o.ctx, &o.tokens).map_err(NoBasis::Config)?;
    let usdt = o
        .tokens
        .usdt(&o.ctx)
        .map_err(|e| NoBasis::Config(e.message))?;
    let settings = graph_settings(&o.ctx);
    match o.tokens.decimals(&o.ctx, usdt).await {
        Ok(usdt_dec) => {
            let basis = Basis {
                settings,
                usdt,
                usdt_dec,
            };
            Ok((basis, per_edge, None))
        }
        Err(e) => match prev {
            Some(b) if b.settings == settings && b.usdt == usdt => {
                Ok((b.clone(), per_edge, Some(e.message)))
            }
            _ => Err(NoBasis::Transient(e.message)),
        },
    }
}

/// Синхронные проверки настроек (как у build): глубина, dex.route_max_pools_per_edge > 0, гистерезис. Ошибки (текст).
pub(super) fn checks(ctx: &Ctx, tokens: &Tokens) -> Result<usize, String> {
    if let Some(name) = depth_book::missing_setting(ctx) {
        return Err(format!("не задана {name}"));
    }
    let n = ctx
        .int("dex.route_max_pools_per_edge")
        .map_err(|e| e.message)?;
    if n == 0 {
        return Err("dex.route_max_pools_per_edge должна быть больше нуля".into());
    }
    Book::prep(ctx, tokens).map(|_| usize::try_from(n).unwrap_or(usize::MAX))
}
