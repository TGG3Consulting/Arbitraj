//! Проверка исполнимой конверсии USDC/USDT на выбранной исходной бирже (Base).
//! GET /api/venues/{id}/base-usdc-conversion?side=Buy|Sell&quantity=<USDC>. Ордер не создаётся.
use axum::{Json, extract::{Path, Query, State}};
use rust_decimal::Decimal;
use serde::Deserialize;

use super::state::{ApiError, AppState};
use crate::core::{settlement::{self, Conversion}, types::{Side, VenueId}};

#[derive(Deserialize)]
pub struct Request {
    side: Side,
    quantity: Decimal,
}

/// Сначала сверяет направление, сеть и монету, затем получает стакан и комиссию аккаунта.
pub async fn quote(State(st): State<AppState>, Path(id): Path<String>, Query(q): Query<Request>)
    -> Result<Json<Conversion>, ApiError>
{
    if q.quantity <= Decimal::ZERO {
        return Err(ApiError::bad("Количество USDC должно быть положительным"));
    }
    settlement::preview(&st.core, &VenueId(id), q.side, q.quantity)
        .await.map(Json).map_err(ApiError::no_data)
}
