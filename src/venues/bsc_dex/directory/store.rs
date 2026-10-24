//! Файл: venues/bsc_dex/directory/store.rs
//! Что делает: справочник пулов в SQLite (storage/venue_state.rs), чтобы после перезапуска не спрашивать индексатор
//! заново раньше index.refresh_s: по монете — когда спрошено, сколько страниц, пулы как отдал индексатор; список DEX
//! сети — когда получен и сам список. Ключ — id сети индексатора и адрес монеты: смена сети — другой справочник.
//! Ошибки запроса не сохраняются (после перезапуска монета просто спрашивается заново).
//! Кто вызывает: directory/refresh.rs.
//! От чего зависит: ctx.rs, storage/venue_state.rs, directory/mod.rs (TokenEntry), core/types (IndexerDex).
//! Настройки: нет.

use alloy_primitives::Address;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::types::IndexerDex;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;
use crate::storage::venue_state;

use super::super::ctx::{Ctx, ID};
use super::TokenEntry;
use super::gt_parse::IndexedPool;

/// Вид записи монеты (id — «сеть:адрес»).
const TOKEN: &str = "index_token";
/// Вид записи списка DEX (id — сеть).
const DEXES: &str = "index_dexes";

#[derive(Serialize, Deserialize)]
struct TokenRow {
    ticker: String,
    fetched_at: DateTime<Utc>,
    pages: u32,
    pools: Vec<IndexedPool>,
}

#[derive(Serialize, Deserialize)]
struct DexRow {
    at: DateTime<Utc>,
    dexes: Vec<IndexerDex>,
}

/// Ошибка SQLite справочника. Принимает: текст. Возвращает: GatewayError Internal.
fn internal(ctx: &Ctx, e: impl std::fmt::Display) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::Internal,
        format!("справочник пулов в SQLite: {e}"),
    )
}

/// Сохраняет ответ по монете. Принимает: сеть, монету, запись (fetched_at обязателен). Ошибки: Internal.
pub async fn save_token(ctx: &Ctx, net: &str, token: Address, t: &TokenEntry) -> GatewayResult<()> {
    let Some(at) = t.fetched_at else {
        return Ok(());
    };
    let row = TokenRow {
        ticker: t.ticker.clone(),
        fetched_at: at,
        pages: t.pages,
        pools: t.pools.clone(),
    };
    let body = serde_json::to_string(&row).map_err(|e| internal(ctx, e))?;
    venue_state::save(
        ctx.journal.db(),
        ID,
        TOKEN,
        &format!("{net}:{token:#x}"),
        body,
    )
    .await
    .map_err(|e| internal(ctx, e))
}

/// Все сохранённые монеты сети: адрес → запись. Повреждённая строка пропускается с записью в журнал. Ошибки: Internal.
pub async fn load_tokens(ctx: &Ctx, net: &str) -> GatewayResult<Vec<(Address, TokenEntry)>> {
    let rows = venue_state::list(ctx.journal.db(), ID, TOKEN, &format!("{net}:"))
        .await
        .map_err(|e| internal(ctx, e))?;
    let mut out = Vec::new();
    for (id, body) in rows {
        let addr = id
            .rsplit(':')
            .next()
            .and_then(|a| a.parse::<Address>().ok());
        let row = serde_json::from_str::<TokenRow>(&body);
        if let Err(e) = &row {
            let msg = format!(
                "Справочник пулов в SQLite: запись {id} повреждена ({e}) — монета будет спрошена заново"
            );
            ctx.log(Level::Warn, "dex.index", msg, None).await;
        }
        if let (Some(addr), Ok(r)) = (addr, row) {
            let e = TokenEntry {
                ticker: r.ticker,
                fetched_at: Some(r.fetched_at),
                tried_at: None,
                pages: r.pages,
                pools: r.pools,
                error: None,
            };
            out.push((addr, e));
        }
    }
    Ok(out)
}

/// Сохраняет список DEX сети. Ошибки: Internal.
pub async fn save_dexes(
    ctx: &Ctx,
    net: &str,
    at: DateTime<Utc>,
    dexes: &[IndexerDex],
) -> GatewayResult<()> {
    let row = DexRow {
        at,
        dexes: dexes.to_vec(),
    };
    let body = serde_json::to_string(&row).map_err(|e| internal(ctx, e))?;
    venue_state::save(ctx.journal.db(), ID, DEXES, net, body)
        .await
        .map_err(|e| internal(ctx, e))
}

/// Сохранённый список DEX сети. None — не сохранялся или повреждён. Ошибки: Internal.
pub async fn load_dexes(
    ctx: &Ctx,
    net: &str,
) -> GatewayResult<Option<(DateTime<Utc>, Vec<IndexerDex>)>> {
    let body = venue_state::load(ctx.journal.db(), ID, DEXES, net)
        .await
        .map_err(|e| internal(ctx, e))?;
    let Some(b) = body else { return Ok(None) };
    match serde_json::from_str::<DexRow>(&b) {
        Ok(r) => Ok(Some((r.at, r.dexes))),
        Err(e) => {
            let msg = format!(
                "Справочник пулов в SQLite: список DEX сети {net} повреждён ({e}) — будет спрошен заново"
            );
            ctx.log(Level::Warn, "dex.index", msg, None).await;
            Ok(None)
        }
    }
}
