//! Файл: api/coins.rs
//! Что делает: экран «Монеты» — справочник монет с налогами, пулами, крючками и чёрным списком
//! и кнопка «Перепроверить» (налоги и крючки сейчас, шлюз `recheck_coins`).
//! Кто вызывает: интерфейс (экран «Монеты»).
//! От чего зависит: api/state.rs, core (Core::coins_json), gateway/registry.rs (pool_depths — решение 73), core/types (Asset, VenueId), storage/journal.rs.
//! Настройки: нет; ручной чёрный список — обычный PUT /api/settings (coin.blocked, coin.blocked_note).
//!
//! GET  /api/coins          — список монет (контракт — src/api/README.md, «Экран Монеты»)
//! GET  /api/coins/{address} — t84: строка одной монеты по адресу контракта, ровно как в списке (обновление одной
//!      строки экрана по живому сообщению coin_check / pool_check / coin_block); 400 — адрес не разобран, 404 — нет монеты
//! POST /api/coins/recheck  — {venue, assets: [Asset]} → {ok, details}
//! GET  /api/coins/ticker-splits — решение 211: [{address, venues: [[биржа, тикер]]}] — конфликт: один адрес под двумя
//!      тикерами на ОДНОЙ бирже (монета там не торгуется, TickerConflict), только для показа
//! GET  /api/coins/names    — решение 211: [{address, ticker, venues: {биржа: тикер}}] — лёгкий справочник имён монет
//!      по адресу для «Живого экрана» и других экранов (имя «NMT / NETMIND», делистинг по тикеру своей площадки, фильтр)
//!
//! Справочник, налоги, проверки пулов и чёрный список держит ядро (core/coins_view.rs → Core::coins_json).
//! Монета для перепроверки — Asset целиком (тикер + адрес контракта): одинаковый тикер может быть
//! разными токенами (Issue #9).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::ConnectionCheck;
use crate::storage::journal::Level;

use super::state::{ApiError, AppState};

/// Список монет из ядра (формат — src/api/README.md, «Экран Монеты»); в каждый пул — `depth`: что о нём знает адаптер
/// DEX (решение 73: глубина по направлениям в полосе цены, спит ли пул, сверка с квотером, tickSpacing — шлюз
/// `pool_depths`), или `depth_unavailable` — почему адаптер не ответил. Ошибок нет.
pub async fn list(State(st): State<AppState>) -> Json<serde_json::Value> {
    let mut coins = st.core.coins_json();
    let rows = coins.as_array_mut().into_iter().flatten();
    with_depths(&st, rows).await;
    Json(coins)
}

/// t84: строка одной монеты по адресу контракта — ровно как в GET /api/coins (та же строка ядра Core::coin_json и те
/// же `depth` / `depth_unavailable`, что кладёт `list`). Экран «Монеты» обновляет ею одну строку по живому сообщению.
/// Принимает: адрес (0x…, 20 байт). Возвращает: строку монеты.
/// Ошибки: 400 — адрес не разобран; 404 — монеты с таким адресом в справочнике нет.
pub async fn one(
    State(st): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a: alloy_primitives::Address = address
        .parse()
        .map_err(|e| ApiError::bad(format!("Адрес монеты «{address}» не разобран: {e}")))?;
    let mut row = st.core.coin_json(a).ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            format!("Монеты с адресом {a} нет в справочнике"),
        )
    })?;
    with_depths(&st, std::iter::once(&mut row)).await;
    Ok(Json(row))
}

/// Кладёт в каждый пул строк `depth` и `depth_unavailable` (решение 73, шлюз `pool_depths` всех площадок реестра).
/// Принимает: состояние, строки монет. Возвращает: ничего. Ошибок нет: сбой адаптера — в `depth_unavailable`.
async fn with_depths<'a>(st: &AppState, rows: impl Iterator<Item = &'a mut serde_json::Value>) {
    let mut depths = std::collections::HashMap::new();
    let mut failed = std::collections::HashMap::new();
    for id in st.registry.ids().await {
        let Ok(v) = st.registry.get(&id).await else {
            continue;
        };
        match v.pool_depths().await {
            Ok(list) => {
                for d in list {
                    let key = (json_key(&d.venue), json_key(&d.pool));
                    depths.insert(key, d);
                }
            }
            Err(e) if e.code == crate::gateway::errors::ErrorCode::NotApplicable => {}
            Err(e) => {
                failed.insert(json_key(&id), e.message);
            }
        }
    }
    for p in rows.filter_map(|c| c["pools"].as_array_mut()).flatten() {
        let venue = p["venue"].to_string();
        let key = (venue.clone(), p["pool"].to_string());
        p["depth"] = serde_json::to_value(depths.get(&key)).unwrap_or_default();
        p["depth_unavailable"] = serde_json::to_value(failed.get(&venue)).unwrap_or_default();
    }
}

/// Ключ сопоставления «пул ядра ↔ пул адаптера»: JSON значения (тот же вид, что в ответе ядра). Ошибок нет.
fn json_key(v: &impl serde::Serialize) -> String {
    serde_json::to_value(v)
        .map(|j| j.to_string())
        .unwrap_or_default()
}

/// Тело «Перепроверить».
#[derive(Deserialize)]
pub struct RecheckBody {
    venue: VenueId,
    assets: Vec<Asset>,
}

/// «Перепроверить» налоги монет и крючки их пулов на площадке DEX — шлюз `recheck_coins`.
/// Результаты придут событиями CoinCheck / PoolCheck (через ядро).
/// Принимает: площадку и монеты (Asset с адресом контракта). Возвращает: {ok, details}; биржа — «не применимо».
/// Ошибки: 400 — пустой список монет. Сбой шлюза — ok:false и причина в details. Пишется в журнал.
pub async fn recheck(
    State(st): State<AppState>,
    Json(b): Json<RecheckBody>,
) -> Result<Json<ConnectionCheck>, ApiError> {
    let assets: Vec<Asset> = b
        .assets
        .into_iter()
        .filter(|a| !a.ticker.trim().is_empty())
        .collect();
    if assets.is_empty() {
        return Err(ApiError::bad("Не указаны монеты для перепроверки"));
    }
    let tickers: Vec<String> = assets.iter().map(|a| a.ticker.clone()).collect();
    let res = match st.registry.get(&b.venue).await {
        Ok(v) => match v.recheck_coins(assets).await {
            Ok(()) => ConnectionCheck {
                ok: true,
                details: "Перепроверка запущена — результаты придут событиями проверки".into(),
            },
            Err(e) => ConnectionCheck {
                ok: false,
                details: e.message,
            },
        },
        Err(e) => ConnectionCheck {
            ok: false,
            details: e.message,
        },
    };
    let level = if res.ok { Level::Info } else { Level::Warn };
    let msg = format!(
        "Перепроверка монет {} на {}: {}",
        tickers.join(", "),
        b.venue.0,
        res.details
    );
    let _ = st
        .journal
        .write(level, "coins.recheck", Some(&b.venue.0), msg, None)
        .await;
    Ok(Json(res))
}

/// Решение 211: имена монет с адресом — показной тикер и тикер каждой площадки (Presence.ticker). Монеты без адреса
/// не входят (не торгуются). Принимает: состояние. Возвращает: JSON-массив. Ошибок нет: справочника нет — пустой массив.
pub async fn names(State(st): State<AppState>) -> Json<serde_json::Value> {
    let coins = crate::core::engine::rd(&st.core.inner.coins);
    let list: Vec<serde_json::Value> = coins
        .all()
        .filter_map(|c| {
            let a = c.asset.bsc_contract?;
            let venues: serde_json::Map<String, serde_json::Value> = c
                .presence
                .iter()
                .map(|p| (p.venue.0.clone(), serde_json::Value::from(p.ticker.clone())))
                .collect();
            Some(serde_json::json!({"address": a, "ticker": c.asset.ticker, "venues": venues}))
        })
        .collect();
    Json(serde_json::Value::Array(list))
}

/// Решение 211: конфликты «один адрес — два тикера на одной бирже» (core/ticker_index.rs), для показа. Ошибок нет.
pub async fn ticker_splits(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(st.core.ticker_splits()).unwrap_or_default())
}
