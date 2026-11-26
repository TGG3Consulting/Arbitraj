//! Восстановление прежнего ордера при переходе от обычного к общему учёту.
use super::*;
use crate::{
    core::types::VenueId,
    storage::{cex_funds_watch::Prior, ledger_db::LedgerDb},
};
use std::collections::HashMap;
/// Сохраняется только при реальном изменении ставки/предела после штатной записи плана.
pub(super) async fn save_adjustment(db: &crate::storage::db::Db, p: &Prior) -> Result<(), String> {
    let key = p.action.clone();
    let body = serde_json::to_string(p).map_err(|e| e.to_string())?;
    let venue = p.request.venue.0.clone();
    let watch = db.ledger().cash_watch.clone();
    db.ledger().write(move|c|{
        let changed=c.execute("INSERT INTO core_state(kind,id,body,updated_at) VALUES('cash_legacy_adjust',?1,?2,?3) ON CONFLICT(kind,id) DO NOTHING",rusqlite::params![key,body,chrono::Utc::now().to_rfc3339()])?;
        if changed!=1{return Err(rusqlite::Error::QueryReturnedNoRows)}
        watch.remember_key(&venue,&key);
        Ok(())
    }).await.map_err(|e|e.to_string())
}
pub(super) async fn unadopted(
    db: &LedgerDb,
    v: &VenueId,
) -> Result<HashMap<String, Prior>, String> {
    let venue = v.0.clone();
    let raw=db.read(move|c|{
        let mut q=c.prepare_cached("SELECT d.body FROM core_state d WHERE kind='cash_legacy_adjust' AND json_extract(d.body,'$.request.venue')=?1 AND NOT EXISTS(SELECT 1 FROM cex_funds WHERE action=d.id AND debit IS NOT NULL) UNION ALL SELECT j.value FROM core_state d INDEXED BY cash_legacy_orders JOIN json_each(d.body,'$.plan.cash_legacy_ioc') j WHERE d.kind='deal' AND json_valid(d.body) AND json_extract(d.body,'$.plan.cash_legacy_ioc') IS NOT NULL AND json_extract(d.body,'$.direction.buy_on')=?1 AND NOT EXISTS(SELECT 1 FROM cex_funds f WHERE f.action=json_extract(j.value,'$.action') AND f.debit IS NOT NULL)")?;
        q.query_map([venue],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()
    }).await.map_err(|e|e.to_string())?;
    let mut out = HashMap::<String, Prior>::new();
    for s in raw {
        let r: Prior = serde_json::from_str(&s).map_err(|e| e.to_string())?;
        if out.get(&r.action).is_none_or(|old| old.fill.is_none()) {
            out.insert(r.action.clone(), r);
        }
    }
    Ok(out)
}
pub(super) async fn adopt(
    core: &Core,
    venue: &dyn Venue,
    cred: &str,
    prior: &Prior,
    accounts: &mut HashMap<String, Bucket>,
) -> GatewayResult<()> {
    let req = &prior.request;
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
    if prior.owner != cred {
        let original = original::load(core.inner.deps.db.ledger(), &prior.action)
            .await
            .map_err(|e| err(ErrorCode::Internal, req, e))?;
        let Some(org) = original else {
            return Err(err(
                ErrorCode::OutcomeUnknown,
                req,
                "Не подтверждён исходный торговый счёт при смене ключа",
            ));
        };
        if org.request.venue != req.venue
            || &org.currency != c
            || org.lookup.client_id != prior.lookup.client_id
        {
            return Err(err(
                ErrorCode::OutcomeUnknown,
                req,
                "Исходный IOC и нативный счёт сохранены неоднозначно",
            ));
        }
        if org.account != b.account {
            return Ok(());
        }
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
