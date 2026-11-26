//! Проверка исполнимой конверсии USDC/USDT на выбранной исходной бирже (Base).
//! GET /api/venues/{id}/base-usdc-conversion?dex=<DEX>&side=Buy|Sell&quantity=<USDC>. Ордер не создаётся.
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
    dex: Option<String>,
}

/// Сначала сверяет направление, сеть и монету, затем получает стакан и комиссию аккаунта.
pub async fn quote(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<Request>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if q.quantity <= Decimal::ZERO {
        return Err(ApiError::bad("Количество USDC должно быть положительным"));
    }
    let market = market(&st, q.dex, &id)?;
    if market.asset.ticker != "USDC" || market.quote != "USDT" {
        return Err(ApiError::bad("Маршрут требует рынка USDC/USDT"));
    }
    let v = settlement::preview(&st.core, &market, q.side, q.quantity)
        .await
        .map_err(ApiError::no_data)?;
    Ok(Json(
        serde_json::json!({"venue":v.venue,"side":v.side,"order_usdc":v.order_amount,"net_usdc":v.net_asset,
        "net_usdt":v.net_quote,"fee_usdc":v.fee_asset,"fee_usdt":v.fee_quote,"limit_price":v.limit_price,"limit_usdt":v.limit_quote}),
    ))
}

#[derive(Deserialize)]
pub struct Lookup {
    deal: String,
    side: Side,
    dex: Option<String>,
}

/// Точный ответ по одному этапу. Частично исполненный ордер и комиссия в третьей монете
/// остаются видны для сверки по ID, даже когда обычный расчёт USDT невозможен.
pub async fn order(
    State(st): State<AppState>,
    Path(venue): Path<String>,
    Query(q): Query<Lookup>,
) -> Result<Json<crate::gateway::settlement_intent::Entry>, ApiError> {
    let market = market(&st, q.dex, &venue)?;
    let row =
        crate::core::settlement_effect::order_record(&st.db, &market, &q.deal, &market.cex, q.side)
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
    pub dex: Option<String>,
    pub quantity: Decimal,
}
/// Модель полного маршрута на заданном количестве с реальными наблюдаемыми условиями.
pub async fn circle(
    State(st): State<AppState>,
    Path(venue): Path<String>,
    Query(q): Query<CircleRequest>,
) -> Result<Json<crate::core::settlement_model::Model>, ApiError> {
    use crate::core::types::Direction;
    let m = market(&st, q.dex, &venue)?;
    let asset = st
        .core
        .coins()
        .all()
        .find(|c| c.asset.chain == m.asset.chain && c.asset.contract == Some(q.token))
        .map(|c| c.asset.clone())
        .ok_or_else(|| ApiError::no_data("Контракта нет в справочнике выбранной сети"))?;
    let cex = m.cex;
    let dex = m.dex;
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

fn market(
    st: &AppState,
    dex: Option<String>,
    cex: &str,
) -> Result<crate::core::settlement_market::Market, ApiError> {
    let dex = explicit_dex(dex)?;
    crate::core::settlement_market::Market::new(&st.core, &dex, &VenueId(cex.into()))
        .map_err(ApiError::no_data)
}

fn explicit_dex(dex: Option<String>) -> Result<VenueId, ApiError> {
    dex.filter(|v| !v.trim().is_empty())
        .map(VenueId)
        .ok_or_else(|| ApiError::bad("Выберите площадку DEX: требуется параметр dex"))
}

/// Котировка для явно запрошенной пары площадок. В ответе присутствуют оба фактических тикера.
pub async fn market_quote(
    State(st): State<AppState>,
    Path((venue, dex)): Path<(String, String)>,
    Query(q): Query<Request>,
) -> Result<Json<Conversion>, ApiError> {
    if q.dex.as_ref().is_some_and(|query| query != &dex) {
        return Err(ApiError::bad(
            "Площадка DEX в пути и параметре запроса расходится",
        ));
    }
    let m = market(&st, Some(dex), &venue)?;
    settlement::preview(&st.core, &m, q.side, q.quantity)
        .await
        .map(Json)
        .map_err(ApiError::no_data)
}

/// Возвращает идентификаторы и первичные факты без обращения к бирже, в том числе после смены реквизитов.
pub async fn intents(
    State(st): State<AppState>,
    Path(deal): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    crate::gateway::settlement_archive::deal(&st.db, &deal)
        .await
        .map(Json)
        .map_err(ApiError::no_data)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conversion_requires_explicit_dex() {
        for input in [None, Some(String::new()), Some(" ".into())] {
            assert!(explicit_dex(input).is_err());
        }
        assert!(explicit_dex(Some("token_market".into())).is_ok_and(|v| v.0 == "token_market"));
    }
}
