//! Файл: core/settlement_effect.rs.
//! Что делает: сверяет заявку по точному подготовленному идентификатору.
//! Кто вызывает: исполнитель конверсии и API.
//! От чего зависит: шлюз, подтверждённая идентичность рынка и ledger.sqlite.
//! Настройки: реквизиты передаются через Market.
use crate::core::types::{TradeRequest, VenueId};
use crate::gateway::{
    contract::Venue,
    errors::{ErrorCode, GatewayError, GatewayResult},
    settlement_intent::{self as log, Entry, Outcome, Request},
};
use crate::storage::db::Db;

use super::settlement_market::Market;

pub(super) fn uncertain(venue: &VenueId, text: impl Into<String>) -> GatewayError {
    GatewayError::new(ErrorCode::OutcomeUnknown, Some(venue.clone()), text)
}

/// Только одинаковая операция может получить результат уже записанного шага.
pub(super) fn same(old: &Request, new: &Request) -> bool {
    matches!((serde_json::to_value(old),serde_json::to_value(new)), (Ok(a), Ok(b)) if a == b)
}

/// Записывает итог только после подтверждения реального идентификатора.
pub(super) async fn commit(
    db: &Db,
    key: &str,
    venue: &VenueId,
    outcome: Outcome,
) -> GatewayResult<Outcome> {
    let entry = log::load(db, key)
        .await
        .map_err(|e| uncertain(venue, e))?
        .ok_or_else(|| uncertain(venue, "Адаптер не сохранил идентификатор до отправки"))?;
    if let Some(done) = entry.result {
        return Ok(done);
    }
    match log::confirm(db, key, &entry, outcome.clone()).await {
        Ok(()) => Ok(outcome),
        Err(reason) => log::load(db, key)
            .await
            .map_err(|e| uncertain(venue, e))?
            .and_then(|row| row.result)
            .ok_or_else(|| uncertain(venue, reason).pending(entry.lookup)),
    }
}

fn order_key(
    m: &Market,
    deal: &str,
    venue: &VenueId,
    side: crate::core::types::Side,
) -> GatewayResult<String> {
    (if m.legacy() {
        serde_json::to_string(&("order", deal, venue, side))
    } else {
        serde_json::to_string(&("order", deal, venue, side, m))
    })
    .map_err(|e| uncertain(venue, e.to_string()))
}

/// Доказательство по ID исходной сделки, стороне и бирже. При ошибке оценки комиссии
/// возвращает фактический ордер целиком, включая валюты всех списаний.
pub async fn order_record(
    db: &Db,
    m: &Market,
    deal: &str,
    venue: &VenueId,
    side: crate::core::types::Side,
) -> GatewayResult<Option<Entry>> {
    let key = order_key(m, deal, venue, side)?;
    log::load(db, &key).await.map_err(|e| uncertain(venue, e))
}

/// Восстанавливает параметры уже отправленного ордера независимо от новых котировок.
pub async fn previous_order(
    db: &Db,
    m: &Market,
    deal: &str,
    venue: &VenueId,
    side: crate::core::types::Side,
) -> GatewayResult<Option<TradeRequest>> {
    let key = order_key(m, deal, venue, side)?;
    match log::load(db, &key).await.map_err(|e| uncertain(venue, e))? {
        None => Ok(None),
        Some(Entry {
            operation: Request::Order(req),
            ..
        }) => Ok(Some(*req)),
        Some(_) => Err(uncertain(venue, "Несовместимая запись ордера конверсии")),
    }
}

/// Один ордер конверсии. Ключ определяется участниками и устойчивым ID этапа, без времени попытки.
pub async fn order(
    db: &Db,
    m: &Market,
    gateway: &dyn Venue,
    req: TradeRequest,
) -> GatewayResult<log::SpotFill> {
    let venue = gateway.id();
    if !gateway.supports_settlement_order() || req.venue != venue || !m.matches(&req.asset) {
        return Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(venue),
            "Нет подтверждаемого пути ордера расчётного токена",
        ));
    }
    let key = order_key(m, &req.deal_id, &venue, req.side)?;
    let operation = Request::Order(Box::new(req.clone()));
    let result = match log::load(db, &key)
        .await
        .map_err(|e| uncertain(&venue, e))?
    {
        Some(entry) => {
            if !gateway.matches_settlement_order(&entry.lookup, &m.asset.ticker, &m.quote) {
                return Err(uncertain(
                    &venue,
                    "Сохранённая заявка относится к другой торговой паре",
                ));
            }
            if !same(&entry.operation, &operation) {
                return Err(uncertain(
                    &venue,
                    "Параметры ордера конверсии совпали с другим сохранённым этапом",
                ));
            }
            if let Some(Outcome::Order(r)) = entry.result {
                return Ok(*r);
            }
            log::capture(async {
                gateway
                    .resolve_trade(req, &entry.lookup)
                    .await?
                    .ok_or_else(|| {
                        uncertain(
                            &venue,
                            "Исход ранее отправленного ордера конверсии ещё не подтверждён",
                        )
                        .pending(entry.lookup)
                    })
            })
            .await?
        }
        None => log::capture(log::scope(db, &key, operation, gateway.trade(req))).await?,
    };
    match commit(db, &key, &venue, Outcome::Order(Box::new(result))).await? {
        Outcome::Order(done) => Ok(*done),
        _ => Err(uncertain(
            &venue,
            "Несовместимый вид сохранённого результата конверсии",
        )),
    }
}

pub use super::settlement_withdrawal::{previous_withdrawal, withdraw};
