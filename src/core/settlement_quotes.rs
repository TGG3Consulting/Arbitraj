//! Файл: core/settlement_quotes.rs.
//! Что делает: получает книгу и условия двух валют.
//! Кто вызывает: API, расчёт маршрута и постановка ордера.
//! От чего зависит: сетевой справочник, правила пары и exec_book.
//! Настройки: venue.quote_asset и сетевые имена площадок.
use super::{
    engine::Core,
    settlement::{Conversion, conversion},
    settlement_market::Market,
    types::{CoinNetworkInfo, Side},
};
use crate::gateway::{events::SpotPairState, frozen::QuoteSnapshot};
use rust_decimal::Decimal;
/// Готовит свежую конверсию из адаптера именно выбранной биржи. Публичного процента комиссии пары
/// для исполнения недостаточно: снимок адаптера содержит комиссию данного аккаунта.
pub async fn preview(
    core: &Core,
    market: &Market,
    side: Side,
    quantity: Decimal,
) -> Result<Conversion, String> {
    let (_, rules, book) = terms(core, market, side).await?;
    conversion(&book, &rules, market, side, quantity).await
}

/// Книга и торговые условия конверсии берутся один раз для расчёта соседних объёмов.
pub(super) async fn terms(
    core: &Core,
    market: &Market,
    side: Side,
) -> Result<(CoinNetworkInfo, SpotPairState, QuoteSnapshot), String> {
    let ce = &market.cex;
    let network = market.network(core, side == Side::Buy)?;
    let rules = super::dirctx_venue::pair_rules(core, ce, market.cex_symbol())?
        .ok_or("Для конверсии требуется спотовая биржа")?;
    let book = super::exec_book::book_on(core, ce, &network.asset, side)
        .await
        .map_err(|e| e.to_string())?;
    Ok((network, rules, book.snap))
}

/// Максимальный исполнимый ордер продажи в пределах имеющихся расчётного актива и ими же уплачиваемой комиссии.
pub(super) async fn sell_within(
    core: &Core,
    market: &Market,
    available: Decimal,
) -> Result<Conversion, String> {
    sell_point(core, market, available)
        .await
        .map_err(|e| e.to_string())
}
pub(super) async fn sell_point(
    core: &Core,
    market: &Market,
    available: Decimal,
) -> Result<Conversion, super::size::Fail> {
    use super::settlement::point_conversion;
    let (_, rules, book) = terms(core, market, Side::Sell).await?;
    let q = point_conversion(&book, &rules, market, Side::Sell, available).await?;
    if q.net_asset <= available {
        return Ok(q);
    }
    let qty = available
        .checked_mul(q.order_amount)
        .and_then(|v| v.checked_div(q.net_asset))
        .ok_or("Не удалось вычислить ордер с комиссией расчётного актива")?;
    let q = point_conversion(&book, &rules, market, Side::Sell, qty).await?;
    if q.net_asset > available {
        return Err("Комиссия продажи превышает доступные расчётного актива".into());
    }
    Ok(q)
}
