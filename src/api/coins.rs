//! Файл: api/coins.rs
//! Что делает: экран «Монеты» — справочник монет с налогами, пулами, крючками и чёрным списком
//! и кнопка «Перепроверить» (налоги и крючки сейчас, шлюз `recheck_coins`).
//! Кто вызывает: интерфейс (экран «Монеты»).
//! От чего зависит: api/state.rs, core (Core::coins_json), gateway/registry.rs, core/types (Asset, VenueId), storage/journal.rs.
//! Настройки: нет; ручной чёрный список — обычный PUT /api/settings (coin.blocked, coin.blocked_note).
//!
//! GET  /api/coins          — список монет (контракт — src/api/README.md, «Экран Монеты»)
//! POST /api/coins/recheck  — {venue, assets: [Asset]} → {ok, details}
//!
//! Справочник, налоги, проверки пулов и чёрный список держит ядро (core/coins_view.rs → Core::coins_json).
//! Монета для перепроверки — Asset целиком (тикер + адрес контракта): одинаковый тикер может быть
//! разными токенами (Issue #9).

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::ConnectionCheck;
use crate::storage::journal::Level;

use super::state::{ApiError, AppState};

/// Список монет из ядра (формат — src/api/README.md, «Экран Монеты»). Ошибок нет.
pub async fn list(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(st.core.coins_json())
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
