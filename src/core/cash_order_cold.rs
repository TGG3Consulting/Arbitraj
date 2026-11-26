//! Восстановление прежнего ордера при переходе от обычного к общему учёту.
use super::*;
use crate::{
    core::types::VenueId,
    storage::{cex_funds_watch::Prior, ledger_db::LedgerDb},
};
use std::collections::HashMap;
pub(super) async fn unadopted(
    db: &LedgerDb,
    v: &VenueId,
) -> Result<HashMap<String, Prior>, String> {
    let venue = v.0.clone();
    db.read(move|c|{
        let mut q=c.prepare_cached("SELECT j.value FROM core_state d INDEXED BY cash_legacy_orders JOIN json_each(d.body,'$.plan.cash_legacy_ioc') j WHERE d.kind='deal' AND json_valid(d.body) AND json_extract(d.body,'$.plan.cash_legacy_ioc') IS NOT NULL AND json_extract(d.body,'$.direction.buy_on')=?1 AND NOT EXISTS(SELECT 1 FROM cex_funds f WHERE f.action=json_extract(j.value,'$.action') AND f.debit IS NOT NULL)")?;
        let rows=q.query_map([venue],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }).await.map_err(|e|e.to_string())?.iter().map(|s|{
        let r:Prior=serde_json::from_str(s).map_err(|e|e.to_string())?;
        Ok((r.action.clone(),r))
    }).collect()
}
pub(super) async fn adopt(
    core: &Core,
    venue: &dyn Venue,
    cred: &str,
    prior: &Prior,
    accounts: &mut HashMap<String, Bucket>,
) -> GatewayResult<()> {
    let req = &prior.request;
    if prior.owner != cred {
        return Err(err(
            ErrorCode::OutcomeUnknown,
            req,
            "Не подтверждён счёт обычного ордера при переключении",
        ));
    }
    let (c, cap) = prior.limit.as_ref().ok_or_else(|| {
        err(
            ErrorCode::OutcomeUnknown,
            req,
            "Нет первоначального предела и валюты обычной покупки",
        )
    })?;
    let b = if let Some(b) = accounts.get(c) {
        b.clone()
    } else {
        let b = crate::gateway::cash_signer::scope(cred.to_owned(), venue.cash_identity(c)).await?;
        accounts.insert(c.clone(), b.clone());
        b
    };
    if b.venue != req.venue.0 || &b.currency != c {
        return Err(err(
            ErrorCode::OutcomeUnknown,
            req,
            "При переключении изменился счёт или валюта обычного ордера",
        ));
    }
    let o = original::Original {
        request: req.clone(),
        lookup: prior.lookup.clone(),
        account: b.account.clone(),
        currency: c.clone(),
    };
    let db = &core.inner.deps.db;
    funds::adopt(db.ledger(), &b, prior.action.clone(), o.clone(), *cap)
        .await
        .map_err(|e| err(ErrorCode::OutcomeUnknown, req, e))?;
    if let Some(fill) = &prior.fill {
        finish(db, &prior.action, req, &o, fill).await?;
    } else {
        // Недоступное чтение первоначального ID оставляет только cap. Внешней отправки нет.
        let (found, gross) = crate::gateway::cash_signer::scope(
            cred.to_owned(),
            intent::capture_optional(async {
                venue
                    .inspect_settlement_order(req.clone(), &prior.lookup)
                    .await?
                    .ok_or_else(|| {
                        err(
                            ErrorCode::OutcomeUnknown,
                            req,
                            "Исход обычной покупки ожидает сверки",
                        )
                    })
            }),
        )
        .await;
        if let (Ok(trade), Some(gross_quote)) = (found, gross) {
            finish(
                db,
                &prior.action,
                req,
                &o,
                &intent::SpotFill { trade, gross_quote },
            )
            .await?;
        }
    }
    Ok(())
}
