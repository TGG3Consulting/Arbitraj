//! Файл: api/coins.rs
//! Что делает: экран «Монеты» — справочник монет с налогами, пулами, крючками и чёрным списком
//! и кнопка «Перепроверить» (налоги и крючки сейчас, шлюз `recheck_coins`).
//! Кто вызывает: интерфейс (экран «Монеты»).
//! От чего зависит: api/state.rs, gateway/registry.rs, core/types (Asset, VenueId), storage/journal.rs.
//! Настройки: нет; ручной чёрный список — обычный PUT /api/settings (coin.blocked, coin.blocked_note).
//!
//! GET  /api/coins          — список монет (контракт — src/api/README.md, «Экран Монеты»)
//! POST /api/coins/recheck  — {venue, tickers} → {ok, details}
//!
//! Справочник, налоги, проверки пулов и чёрный список держит ядро (core/registry.rs, core/blacklist.rs,
//! core/market_state.rs). Ядро к веб-серверу ещё не подключено — GET отвечает 503 «нет данных»,
//! а не пустым списком (пустой список значил бы «монет нет», это неправда).

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::ConnectionCheck;
use crate::storage::journal::Level;

use super::state::{ApiError, AppState};

/// Список монет. Ошибки: 503 — нет данных (источник — ядро, ещё не подключено).
pub async fn list() -> ApiError {
    ApiError::no_data(
        "Нет данных: справочник монет, налоги, пулы и чёрный список ведёт ядро \
         (core/registry.rs, core/blacklist.rs) — оно ещё не подключено к веб-серверу",
    )
}

/// Тело «Перепроверить».
#[derive(Deserialize)]
pub struct RecheckBody {
    venue: VenueId,
    tickers: Vec<String>,
}

/// «Перепроверить» налоги монет и крючки их пулов на площадке DEX — шлюз `recheck_coins`.
/// Результаты придут событиями CoinCheck / PoolCheck (через ядро).
/// Принимает: площадку и тикеры. Возвращает: {ok, details}; биржа — «не применимо» в details.
/// Монета передаётся тикером, без адреса: так задан запрос контракта (адрес знает адаптер по своему справочнику).
/// Ошибки: 400 — пустой список тикеров. Сбой шлюза — ok:false и причина в details. Пишется в журнал.
pub async fn recheck(
    State(st): State<AppState>,
    Json(b): Json<RecheckBody>,
) -> Result<Json<ConnectionCheck>, ApiError> {
    let tickers: Vec<String> = b
        .tickers
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if tickers.is_empty() {
        return Err(ApiError::bad("Не указаны монеты для перепроверки"));
    }
    let assets = tickers
        .iter()
        .map(|t| Asset {
            ticker: t.clone(),
            bsc_contract: None,
        })
        .collect();
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
