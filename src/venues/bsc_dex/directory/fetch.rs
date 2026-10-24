//! Файл: venues/bsc_dex/directory/fetch.rs
//! Что делает: запросы справочника к индексатору: пулы одной монеты по страницам (не больше index.pages_per_token;
//! пустая страница, HTTP 400 на странице > 1 или страница короче index.page_size — конец списка) и список DEX сети по links.next
//! (сохраняется в SQLite и в состояние справочника).
//! Кто вызывает: directory/refresh.rs.
//! От чего зависит: ctx.rs, directory/{gt_http,gt_parse,store,mod}.rs.
//! Настройки: через вызываемые — index.* [bsc_dex].
//!
//! Эндпоинты — https://docs.coingecko.com/demo/reference/top-pools-contract-address (GET …/tokens/{address}/pools,
//! «up to 20 pools per page») и https://docs.coingecko.com/demo/reference/endpoint-overview (GET …/dexes).

use alloy_primitives::Address;
use chrono::Utc;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::gt_http::{self, Got};
use super::gt_parse::{self, IndexedPool};
use super::refresh::stops;
use super::store;

/// Список DEX сети по всем страницам (links.next). Сбой связи — в журнал, список прежний.
/// Ошибки: NotConfigured, SecretsLocked.
pub async fn dexes(ctx: &Ctx, net: &str) -> GatewayResult<()> {
    let mut all = Vec::new();
    let mut page = 1;
    loop {
        let path = format!("/networks/{net}/dexes?page={page}");
        let body = match gt_http::get(ctx, &path, page).await {
            Ok(Got::Body(b)) => b,
            Ok(Got::PageEnd(_)) => break,
            Err(e) if stops(&e) => return Err(e),
            Err(e) => {
                ctx.log(
                    Level::Warn,
                    "dex.index",
                    format!("Список DEX: {}", e.message),
                    None,
                )
                .await;
                return Ok(());
            }
        };
        let (list, next) = match gt_parse::dexes(&body) {
            Ok(v) => v,
            Err(why) => {
                ctx.log(Level::Warn, "dex.index", format!("Список DEX: {why}"), None)
                    .await;
                return Ok(());
            }
        };
        all.extend(list);
        if !next {
            break;
        }
        page += 1;
    }
    let at = Utc::now();
    if let Err(e) = store::save_dexes(ctx, net, at, &all).await {
        ctx.log(Level::Error, "dex.index", e.message, None).await;
    }
    ctx.index.with(|s| (s.dexes_at, s.dexes) = (Some(at), all));
    Ok(())
}

/// Пулы одной монеты по страницам. Принимает: сеть, монету, предел страниц, размер страницы.
/// Возвращает: (пулы, сколько страниц прочитано). Ошибки: запрос любой страницы (пол-списка — не факт), BadResponse.
pub async fn token_pools(
    ctx: &Ctx,
    net: &str,
    token: Address,
    pages: u32,
    size: usize,
) -> GatewayResult<(Vec<IndexedPool>, u32)> {
    let mut out = Vec::new();
    let mut read = 0;
    for page in 1..=pages {
        let path = format!(
            "/networks/{net}/tokens/{token:#x}/pools?page={page}&include=base_token,quote_token,dex"
        );
        let body = match gt_http::get(ctx, &path, page).await? {
            Got::Body(b) => b,
            Got::PageEnd(_) => break,
        };
        let list = gt_parse::pools(&body).map_err(|w| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("{}: {path}: {w}", gt_http::SOURCE),
            )
        })?;
        // Пустая страница — конец списка (как и 400 на странице > 1 и страница короче index.page_size).
        if list.is_empty() {
            break;
        }
        read = page;
        let last = list.len() < size;
        out.extend(list);
        if last {
            break;
        }
    }
    Ok((out, read))
}
