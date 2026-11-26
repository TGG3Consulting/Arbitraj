//! Файл: core/settlement_withdrawal.rs.
//! Что делает: сверяет вывод по сохранённым реквизитам.
//! Кто вызывает: перевод расчётного токена.
//! От чего зависит: API шлюза и журнал идентификаторов.
//! Настройки: рынок и кошелёк задаются поручением.
use super::{
    settlement_effect::{commit, same, uncertain},
    settlement_market::Market,
    types::{CoinNetworkInfo, DexAccountId, Transfer, VenueId, WithdrawRequest},
};
use crate::{
    gateway::{
        contract::Venue,
        errors::{ErrorCode, GatewayError, GatewayResult},
        settlement_intent::{self as log, Entry, Outcome, Request},
    },
    storage::db::Db,
};
fn withdrawal_key(
    m: &Market,
    deal: &str,
    venue: &VenueId,
    to: &VenueId,
    account: Option<&DexAccountId>,
) -> GatewayResult<String> {
    (if m.legacy() {
        serde_json::to_string(&("withdraw", deal, venue, to, account))
    } else {
        serde_json::to_string(&("withdraw", deal, venue, to, account, m))
    })
    .map_err(|e| uncertain(venue, e.to_string()))
}

/// Записанные до POST реквизиты нужны для сверки вывода даже после списания всего баланса.
pub async fn previous_withdrawal(
    db: &Db,
    m: &Market,
    deal: &str,
    venue: &VenueId,
    to: &VenueId,
    account: Option<&DexAccountId>,
) -> GatewayResult<Option<WithdrawRequest>> {
    let key = withdrawal_key(m, deal, venue, to, account)?;
    match log::load(db, &key).await.map_err(|e| uncertain(venue, e))? {
        None => Ok(None),
        Some(Entry {
            operation: Request::Withdrawal(req),
            ..
        }) => Ok(Some(*req)),
        Some(_) => Err(uncertain(venue, "Несовместимая запись вывода конверсии")),
    }
}

/// Один вывод расчётного актива. Успешный результат позволяет обычному наблюдателю продолжить отслеживание зачисления.
pub async fn withdraw(
    db: &Db,
    m: &Market,
    gateway: &dyn Venue,
    req: &WithdrawRequest,
    to: &VenueId,
    account: Option<&DexAccountId>,
    network: Option<&CoinNetworkInfo>,
) -> GatewayResult<Transfer> {
    let venue = gateway.id();
    if !gateway.supports_settlement_withdrawal() || req.venue != venue || !m.matches(&req.asset) {
        return Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(venue),
            "Площадка не сохраняет идентификатор вывода расчётного токена до отправки",
        ));
    }
    let key = withdrawal_key(m, &req.deal_id, &venue, to, account)?;
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
                    "Реквизиты сохранённого вывода конверсии отличаются",
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
                        "Вывод конверсии зарегистрирован, статус ещё не подтверждён",
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
            "Несовместимый вид сохранённого результата конверсии",
        )),
    }
}
