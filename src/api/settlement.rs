//! Проверка исполнимой конверсии USDC/USDT на выбранной исходной бирже (Base).
//! GET /api/venues/{id}/base-usdc-conversion?side=Buy|Sell&quantity=<USDC>. Ордер не создаётся.
use axum::{
    Json,
    extract::{Path, Query, State},
};
use rust_decimal::Decimal;
use serde::Deserialize;

use super::state::{ApiError, AppState};
use crate::core::{
    settlement::{self, Conversion},
    types::{Side, VenueId},
};

#[derive(Deserialize)]
pub struct Request {
    side: Side,
    quantity: Decimal,
}

/// Сначала сверяет направление, сеть и монету, затем получает стакан и комиссию аккаунта.
pub async fn quote(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<Request>,
) -> Result<Json<Conversion>, ApiError> {
    if q.quantity <= Decimal::ZERO {
        return Err(ApiError::bad("Количество USDC должно быть положительным"));
    }
    settlement::preview(&st.core, &VenueId(id), q.side, q.quantity)
        .await
        .map(Json)
        .map_err(ApiError::no_data)
}

#[derive(Deserialize)]
pub struct Lookup {
    deal: String,
    side: Side,
}

/// Точный ответ по одному этапу. Частично исполненный ордер и комиссия в третьей монете
/// остаются видны для сверки по ID, даже когда обычный расчёт USDT невозможен.
pub async fn order(
    State(st): State<AppState>,
    Path(venue): Path<String>,
    Query(q): Query<Lookup>,
) -> Result<Json<crate::gateway::settlement_intent::Entry>, ApiError> {
    let row =
        crate::core::settlement_effect::order_record(&st.db, &q.deal, &VenueId(venue), q.side)
            .await
            .map_err(|e| ApiError::no_data(e.message))?
            .ok_or_else(|| ApiError::no_data("Сохранённого ордера Base с этими реквизитами нет"))?;
    Ok(Json(row))
}

/// Возвращает исходное поручение и фактические остатки по каждому направлению сделки.
pub async fn paths(
    State(st): State<AppState>,
    Path(deal): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let paths = crate::core::settlement_path::rows(&st.db, Some(&deal))
        .await
        .map_err(ApiError::no_data)?;
    Ok(Json(
        serde_json::to_value(paths.iter().map(|p| (p, p.progress())).collect::<Vec<_>>())
            .map_err(|e| ApiError::no_data(e.to_string()))?,
    ))
}

#[derive(Deserialize)]
pub struct CircleRequest {
    pub token: alloy_primitives::Address,
    /// Сторона Base: Buy — финансирование на бирже перед целевой покупкой, Sell — возврат выручки на биржу.
    pub base_side: Side,
    pub quantity: Decimal,
}
/// Модель полного маршрута на заданном количестве с реальными наблюдаемыми условиями.
pub async fn circle(
    State(st): State<AppState>,
    Path(venue): Path<String>,
    Query(q): Query<CircleRequest>,
) -> Result<Json<crate::core::settlement_model::Model>, ApiError> {
    use crate::core::types::Direction;
    let asset = st
        .core
        .coins()
        .all()
        .find(|c| {
            c.asset.chain.as_ref().is_some_and(|n| n.as_str() == "base")
                && c.asset.contract == Some(q.token)
        })
        .map(|c| c.asset.clone())
        .ok_or_else(|| ApiError::no_data("Контракта Base нет в справочнике монет"))?;
    let cex = VenueId(venue);
    let dex = VenueId("base_dex".into());
    let (buy_on, sell_on) = if q.base_side == Side::Buy {
        (dex, cex)
    } else {
        (cex, dex)
    };
    crate::core::settlement_model::evaluate(
        &st.core,
        &Direction {
            asset,
            buy_on,
            sell_on,
        },
        q.quantity,
    )
    .await
    .map(Json)
    .map_err(ApiError::no_data)
}
