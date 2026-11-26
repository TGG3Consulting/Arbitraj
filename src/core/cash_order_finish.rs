//! Файл: core/cash_order_finish.rs.
//! Назначение: сверяет исходный accountId и окончательное фактическое списание с комиссией.
//! Потребители: обычное завершение и холодное восстановление смешанной покупки.
//! Зависимости: нативный идентификатор биржи и сохранённое финансовое действие.
//! Настройки: отсутствуют.
use super::*;
/// Старый смешанный шаг проверяется чтением по исходному ID в своём исполнителе.
pub(crate) async fn recovered(
    core: &Core,
    venue: &dyn Venue,
    outer: &str,
    fill: &intent::SpotFill,
) -> GatewayResult<()> {
    let req = &fill.trade.request;
    if req.side != Side::Buy || !venue.supports_cash_reservation() {
        return Ok(());
    }
    let key = cash_key(req, 1, Some(outer))?;
    let db = &core.inner.deps.db;
    let Some(o) = original::load(db.ledger(), &key)
        .await
        .map_err(|e| err(ErrorCode::Internal, req, e))?
    else {
        return Ok(());
    };
    let signer = venue.settlement_account()?;
    let bucket =
        crate::gateway::cash_signer::scope(signer, venue.cash_identity(&o.currency)).await?;
    if !original::binding(&o, &bucket) {
        return Err(err(
            ErrorCode::OutcomeUnknown,
            req,
            "Счёт исходного ордера изменился",
        ));
    }
    finish(db, &key, req, &o, fill).await
}
pub(super) async fn finish(
    db: &crate::storage::db::Db,
    key: &str,
    req: &TradeRequest,
    o: &original::Original,
    fill: &intent::SpotFill,
) -> GatewayResult<()> {
    let b = Bucket {
        venue: req.venue.0.clone(),
        account: o.account.clone(),
        currency: o.currency.clone(),
    };
    let mut debit = fill.gross_quote;
    for fee in &fill.trade.fees {
        if fee.amount < Decimal::ZERO {
            return Err(err(
                ErrorCode::OutcomeUnknown,
                req,
                "Отрицательная комиссия в сверке IOC",
            ));
        }
        if fee.asset == b.currency {
            debit = debit
                .checked_add(fee.amount)
                .ok_or_else(|| err(ErrorCode::OutcomeUnknown, req, "Дебет IOC вне диапазона"))?;
        }
    }
    let client = o
        .lookup
        .client_id
        .as_deref()
        .ok_or_else(|| err(ErrorCode::OutcomeUnknown, req, "Нет сохранённого client ID"))?;
    funds::finish(db.ledger(), &b, key, client, &fill.trade.venue_ref, debit)
        .await
        .map_err(|e| err(ErrorCode::OutcomeUnknown, req, e))?;
    Ok(())
}

/// Штатный poll после потерянного ответа. При смене аккаунта отправка/отмена не выполняется.
pub(crate) async fn resolve(
    core: &Core,
    venue: &dyn Venue,
    req: &TradeRequest,
    p: &PendingRef,
    action: &str,
) -> GatewayResult<Option<TradeResult>> {
    let db = &core.inner.deps.db;
    let o = original::load(db.ledger(), action)
        .await
        .map_err(|e| err(ErrorCode::Internal, req, e))?
        .ok_or_else(|| {
            err(
                ErrorCode::OutcomeUnknown,
                req,
                "Нет исходной записи покупки",
            )
        })?;
    if o.request.deal_id != req.deal_id
        || o.request.side != req.side
        || o.request.venue != req.venue
        || o.lookup.client_id != p.client_id
    {
        return Err(err(
            ErrorCode::OutcomeUnknown,
            req,
            "Исходный клиентский ID и параметры покупки не совпадают",
        ));
    }
    let signer = venue.settlement_account()?;
    crate::gateway::cash_signer::scope(signer, async {
        let b = venue.cash_identity(&o.currency).await?;
        if !original::binding(&o, &b) {
            return Err(err(
                ErrorCode::OutcomeUnknown,
                req,
                "Изменился нативный счёт исходного ордера",
            ));
        }
        let mut lookup = o.lookup.clone();
        if let Some(id) = &p.venue_ref {
            if lookup.venue_ref.as_ref().is_some_and(|known| known != id) {
                return Err(err(
                    ErrorCode::OutcomeUnknown,
                    req,
                    "Изменился биржевой ID исходного ордера",
                ));
            }
            lookup.venue_ref = Some(id.clone());
        }
        let mut seen = false;
        let result = intent::capture(async {
            let r = venue
                .inspect_settlement_order(o.request.clone(), &lookup)
                .await?;
            if r.is_some() {
                seen = true;
            }
            r.ok_or_else(|| {
                err(
                    ErrorCode::OutcomeUnknown,
                    req,
                    "Исходный IOC ещё не подтверждён",
                )
            })
        })
        .await;
        match result {
            Ok(fill) => {
                finish(db, action, req, &o, &fill).await?;
                Ok(Some(fill.trade))
            }
            Err(e) if !seen && e.code == ErrorCode::OutcomeUnknown => Ok(None),
            Err(e) => Err(e),
        }
    })
    .await
}
