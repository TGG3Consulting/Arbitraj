//! Восстановление денежной операции Base USDC по фактическому подготовленному идентификатору.
//! При повторном входе используется сохранённый запрос и подтверждение площадки. Отсутствующий
//! в истории ордер или вывод после потери ответа остаётся незавершённым.
use crate::core::types::{
    Asset, CoinNetworkInfo, DexAccountId, TradeRequest, TradeResult, Transfer, VenueId,
    WithdrawRequest,
};
use crate::gateway::{
    contract::Venue,
    errors::{ErrorCode, GatewayError, GatewayResult},
    settlement_intent::{self as log, Entry, Outcome, Request},
};
use crate::storage::db::Db;

/// Точная идентичность токена, для которого требуется конверсия.
pub fn base_usdc(asset: &Asset) -> bool {
    asset.chain.as_ref().is_some_and(|c| c.as_str() == "base")
        && asset.contract == Some(super::settlement::BASE_USDC)
        && asset.ticker == "USDC"
}

fn uncertain(venue: &VenueId, text: impl Into<String>) -> GatewayError {
    GatewayError::new(ErrorCode::OutcomeUnknown, Some(venue.clone()), text)
}

/// Только одинаковая операция может получить результат уже записанного шага.
fn same(old: &Request, new: &Request) -> bool {
    matches!((serde_json::to_value(old),serde_json::to_value(new)), (Ok(a), Ok(b)) if a == b)
}

/// Записывает итог только после подтверждения реального идентификатора.
async fn commit(db: &Db, key: &str, venue: &VenueId, outcome: Outcome) -> GatewayResult<()> {
    let entry = log::load(db, key)
        .await
        .map_err(|e| uncertain(venue, e))?
        .ok_or_else(|| uncertain(venue, "Адаптер не сохранил идентификатор до отправки"))?;
    if entry.result.is_some() {
        return Ok(());
    }
    log::confirm(db, key, &entry, outcome)
        .await
        .map_err(|e| uncertain(venue, e).pending(entry.lookup))
}

/// Один ордер конверсии. Ключ определяется участниками и устойчивым ID этапа, без времени попытки.
pub async fn order(db: &Db, gateway: &dyn Venue, req: TradeRequest) -> GatewayResult<TradeResult> {
    let venue = gateway.id();
    if !gateway.supports_settlement_order() || req.venue != venue || !base_usdc(&req.asset) {
        return Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(venue),
            "Нет подтверждаемого пути ордера Base USDC",
        ));
    }
    let key = serde_json::to_string(&("order", &req.deal_id, &venue, req.side))
        .map_err(|e| uncertain(&venue, e.to_string()))?;
    let operation = Request::Order(Box::new(req.clone()));
    let result = match log::load(db, &key)
        .await
        .map_err(|e| uncertain(&venue, e))?
    {
        Some(entry) => {
            if !same(&entry.operation, &operation) {
                return Err(uncertain(
                    &venue,
                    "Параметры ордера Base совпали с другим сохранённым этапом",
                ));
            }
            if let Some(Outcome::Order(r)) = entry.result {
                return Ok(*r);
            }
            gateway
                .resolve_trade(req, &entry.lookup)
                .await?
                .ok_or_else(|| {
                    uncertain(
                        &venue,
                        "Исход ранее отправленного ордера Base ещё не подтверждён",
                    )
                    .pending(entry.lookup)
                })?
        }
        None => log::scope(db, &key, operation, gateway.trade(req)).await?,
    };
    commit(db, &key, &venue, Outcome::Order(Box::new(result.clone()))).await?;
    Ok(result)
}

/// Один вывод USDC. Успешный результат позволяет обычному наблюдателю продолжить отслеживание зачисления.
pub async fn withdraw(
    db: &Db,
    gateway: &dyn Venue,
    req: &WithdrawRequest,
    to: &VenueId,
    account: Option<&DexAccountId>,
    network: Option<&CoinNetworkInfo>,
) -> GatewayResult<Transfer> {
    let venue = gateway.id();
    if !gateway.supports_settlement_withdrawal() || req.venue != venue || !base_usdc(&req.asset) {
        return Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(venue),
            "Площадка не сохраняет идентификатор вывода Base USDC до отправки",
        ));
    }
    let key = serde_json::to_string(&("withdraw", &req.deal_id, &venue, to, account))
        .map_err(|e| uncertain(&venue, e.to_string()))?;
    let operation = Request::Withdrawal(Box::new(req.clone()));
    let result = match log::load(db, &key)
        .await
        .map_err(|e| uncertain(&venue, e))?
    {
        Some(Entry {
            operation: old,
            lookup,
            result,
        }) => {
            if !same(&old, &operation) {
                return Err(uncertain(
                    &venue,
                    "Реквизиты сохранённого вывода Base отличаются",
                ));
            }
            if let Some(Outcome::Withdrawal(r)) = result {
                return Ok(*r);
            }
            gateway
                .resolve_withdraw(req.clone(), &lookup)
                .await?
                .ok_or_else(|| {
                    uncertain(
                        &venue,
                        "Вывод Base зарегистрирован, статус ещё не подтверждён",
                    )
                    .pending(lookup)
                })?
        }
        None => {
            log::scope(db, &key, operation, async {
                match network {
                    Some(info) => gateway.withdraw_with_network(req.clone(), info).await,
                    None => gateway.withdraw(req.clone()).await,
                }
            })
            .await?
        }
    };
    commit(
        db,
        &key,
        &venue,
        Outcome::Withdrawal(Box::new(result.clone())),
    )
    .await?;
    Ok(result)
}
