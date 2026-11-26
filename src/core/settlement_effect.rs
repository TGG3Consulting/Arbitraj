//! Восстановление денежной операции Base USDC по фактическому подготовленному идентификатору.
//! При повторном входе используется сохранённый запрос и подтверждение площадки. Отсутствующий
//! в истории ордер или вывод после потери ответа остаётся незавершённым.
use crate::core::types::{
    Asset, CoinNetworkInfo, DexAccountId, TradeRequest, Transfer, VenueId, WithdrawRequest,
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
async fn commit(db: &Db, key: &str, venue: &VenueId, outcome: Outcome) -> GatewayResult<Outcome> {
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

fn order_key(deal: &str, venue: &VenueId, side: crate::core::types::Side) -> GatewayResult<String> {
    serde_json::to_string(&("order", deal, venue, side))
        .map_err(|e| uncertain(venue, e.to_string()))
}

/// Доказательство по ID исходной сделки, стороне и бирже. При ошибке оценки комиссии
/// возвращает фактический ордер целиком, включая валюты всех списаний.
pub async fn order_record(
    db: &Db,
    deal: &str,
    venue: &VenueId,
    side: crate::core::types::Side,
) -> GatewayResult<Option<Entry>> {
    let key = order_key(deal, venue, side)?;
    log::load(db, &key).await.map_err(|e| uncertain(venue, e))
}

/// Восстанавливает параметры уже отправленного ордера независимо от новых котировок.
pub async fn previous_order(
    db: &Db,
    deal: &str,
    venue: &VenueId,
    side: crate::core::types::Side,
) -> GatewayResult<Option<TradeRequest>> {
    let key = order_key(deal, venue, side)?;
    match log::load(db, &key).await.map_err(|e| uncertain(venue, e))? {
        None => Ok(None),
        Some(Entry {
            operation: Request::Order(req),
            ..
        }) => Ok(Some(*req)),
        Some(_) => Err(uncertain(venue, "Несовместимая запись ордера Base")),
    }
}

/// Один ордер конверсии. Ключ определяется участниками и устойчивым ID этапа, без времени попытки.
pub async fn order(
    db: &Db,
    gateway: &dyn Venue,
    req: TradeRequest,
) -> GatewayResult<log::SpotFill> {
    let venue = gateway.id();
    if !gateway.supports_settlement_order() || req.venue != venue || !base_usdc(&req.asset) {
        return Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(venue),
            "Нет подтверждаемого пути ордера Base USDC",
        ));
    }
    let key = order_key(&req.deal_id, &venue, req.side)?;
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
            log::capture(async {
                gateway
                    .resolve_trade(req, &entry.lookup)
                    .await?
                    .ok_or_else(|| {
                        uncertain(
                            &venue,
                            "Исход ранее отправленного ордера Base ещё не подтверждён",
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
            "Несовместимый вид сохранённого результата Base",
        )),
    }
}

fn withdrawal_key(
    deal: &str,
    venue: &VenueId,
    to: &VenueId,
    account: Option<&DexAccountId>,
) -> GatewayResult<String> {
    serde_json::to_string(&("withdraw", deal, venue, to, account))
        .map_err(|e| uncertain(venue, e.to_string()))
}

/// Записанные до POST реквизиты нужны для сверки вывода даже после списания всего баланса.
pub async fn previous_withdrawal(
    db: &Db,
    deal: &str,
    venue: &VenueId,
    to: &VenueId,
    account: Option<&DexAccountId>,
) -> GatewayResult<Option<WithdrawRequest>> {
    let key = withdrawal_key(deal, venue, to, account)?;
    match log::load(db, &key).await.map_err(|e| uncertain(venue, e))? {
        None => Ok(None),
        Some(Entry {
            operation: Request::Withdrawal(req),
            ..
        }) => Ok(Some(*req)),
        Some(_) => Err(uncertain(venue, "Несовместимая запись вывода Base")),
    }
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
    let key = withdrawal_key(&req.deal_id, &venue, to, account)?;
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
    match commit(db, &key, &venue, Outcome::Withdrawal(Box::new(result))).await? {
        Outcome::Withdrawal(done) => Ok(*done),
        _ => Err(uncertain(
            &venue,
            "Несовместимый вид сохранённого результата Base",
        )),
    }
}
