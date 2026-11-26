//! Файл: core/settlement_request.rs.
//! Что делает: выбирает исходное поручение либо проверяет свежую книгу и лимит нового ордера.
//! Кто вызывает: исполнитель полного денежного маршрута.
//! От чего зависит: точные реквизиты сохранённого ордера, глубина, правила пары.
//! Настройки: пределы покупки и продажи из решения о входе.
use super::{
    engine::Core,
    settlement_cycle::Mandate,
    settlement_target as target,
    types::{Side, TradeRequest},
};
use rust_decimal::Decimal;
pub(super) async fn request(
    core: &Core,
    id: &str,
    p: &Mandate,
    side: Side,
    qty: Decimal,
    limit: Decimal,
) -> Result<TradeRequest, String> {
    let d = &p.direction;
    let sell = side == Side::Sell;
    let venue = if sell { &d.sell_on } else { &d.buy_on };
    if let Some(old) = target::previous(core, &p.market, id, &p.wallet, venue, side).await? {
        return Ok(old);
    }
    if qty <= Decimal::ZERO {
        return Err("Для целевого ордера нет положительного остатка".into());
    }
    let q = super::costs_quote::fetch(core, d, sell, qty, None)
        .await
        .map_err(|e| e.to_string())?;
    let worst = super::costs_quote::worst(core, venue, &d.asset, side, qty, q.avg_price, None)
        .map_err(|e| e.to_string())?;
    if q.fillable_qty < qty || (sell && worst < limit) || (!sell && worst > limit) {
        return Err("Целевой ордер вышел за подтверждённую глубину или цену".into());
    }
    let request = TradeRequest {
        venue: venue.clone(),
        asset: super::venue_ticker::on_venue(core, &d.asset, venue),
        side,
        qty,
        worst_price: worst,
        deal_id: id.into(),
        route: q.route,
        exit_position: sell,
        remeasure: None,
    };
    let gw = core
        .inner
        .deps
        .gw
        .get_as(venue, Some(&p.wallet))
        .await
        .map_err(|e| e.message)?;
    if gw.kind() == crate::gateway::contract::VenueKind::Cex {
        gw.check_order(&request).await.map_err(|e| e.message)?;
    }
    Ok(request)
}
