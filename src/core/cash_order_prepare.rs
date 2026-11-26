//! Файл: core/cash_order_prepare.rs.
//! Назначение: атомарно регистрирует лимит, client ID и данные исходного запроса перед отправкой.
//! Потребители: settlement_intent::before_send под действующим контекстом денежной покупки.
//! Зависимости: текущий баланс подтверждённого аккаунта и SQLite.
//! Настройки: выключатели исходного исполнения проверяются у того же поручения.
use super::*;
/// Единственное место допуска после округления параметров и получения фактического client ID.
pub fn active() -> bool {
    ACTIVE.try_with(|_| ()).is_ok()
}
pub async fn before_send(
    lookup: &PendingRef,
    outer: Option<(String, String)>,
) -> GatewayResult<()> {
    let Ok(a) = ACTIVE.try_with(Clone::clone) else {
        return Ok(());
    };
    let db = a.core.inner.deps.db.ledger();
    let rev = original::revisions(db, &a.req.venue.0, &a.currency)
        .await
        .map_err(|e| err(ErrorCode::Internal, &a.req, e))?;
    let (mut balance, cap) = a.venue.cash_balance(&a.req, lookup, &a.currency).await?;
    let bucket = balance.bucket.clone();
    if bucket.venue != a.req.venue.0
        || bucket.currency != a.currency
        || a.credentials != a.venue.settlement_account()?
    {
        return Err(err(
            ErrorCode::NotConfigured,
            &a.req,
            "Баланс получен для других реквизитов",
        ));
    }
    balance.epoch = rev.get(&bucket.account).copied().unwrap_or_default();
    let client = lookup.client_id.clone().ok_or_else(|| {
        err(
            ErrorCode::BadResponse,
            &a.req,
            "Отсутствует client ID ордера",
        )
    })?;
    let row = original::Original {
        account: bucket.account.clone(),
        currency: bucket.currency.clone(),
        request: a.req.clone(),
        lookup: lookup.clone(),
    };
    let held_bucket = bucket.clone();
    let own = a.req.deal_id.clone();
    let owner = a.credentials.clone();
    let key = a.key.clone();
    let marker = outer.clone();
    let action = a.key.clone();
    let client_for_abort = client.clone();
    let claim = Claim {
        action: a.key,
        client,
        deal: a.req.deal_id.clone(),
        funding: a.funding,
        cap,
    };
    match funds::admit_recorded(
        db,
        &bucket,
        claim,
        balance,
        move |c| crate::core::cash_holds::read_for(c, &held_bucket, Some(&own), Some(&owner)),
        move |c| {
            original::record(c, &key, &row)?;
            if let Some((outer_key, body)) = outer {
                intent::insert_cash_marker(c, &outer_key, &body)?;
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| err(ErrorCode::Internal, &a.req, e))?
    {
        Admission::Send => {
            let bound = intent::verify_cash_marker(lookup).await.and_then(|()| {
                if a.credentials != a.venue.settlement_account()? {
                    return Err(err(
                        ErrorCode::NotConfigured,
                        &a.req,
                        "Сменились реквизиты биржи перед отправкой",
                    ));
                }
                Ok(())
            });
            if let Err(reason) = bound {
                original::abort(db, &bucket, &action, &client_for_abort, marker)
                    .await
                    .map_err(|e| err(ErrorCode::Internal, &a.req, e))?;
                return Err(reason);
            }
            *a.owned.lock().unwrap_or_else(|e| e.into_inner()) = Some(Owned {
                bucket,
                client: client_for_abort,
                marker,
            });
            Ok(())
        }
        Admission::Recheck => Err(err(
            ErrorCode::StaleData,
            &a.req,
            "Остаток изменился, требуется новая предпроверка",
        )),
        Admission::Insufficient => Err(err(
            ErrorCode::InsufficientBalance,
            &a.req,
            "После резервов счёта недостаточно подтверждённых средств",
        )),
        Admission::Inspect(_) => Err(err(
            ErrorCode::OutcomeUnknown,
            &a.req,
            "Исходная покупка уже зарегистрирована для этого денежного шага",
        )),
    }
}

/// Единственный исполнитель проверил, что транспорт не пытался отправить POST даже на первый URL.
pub(crate) async fn not_dispatched() -> GatewayResult<()> {
    if let Ok(a) = ACTIVE.try_with(Clone::clone) {
        let held = a.owned.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(held) = held {
            original::abort(
                a.core.inner.deps.db.ledger(),
                &held.bucket,
                &a.key,
                &held.client,
                held.marker,
            )
            .await
            .map_err(|e| err(ErrorCode::Internal, &a.req, e))?;
        }
    }
    Ok(())
}
