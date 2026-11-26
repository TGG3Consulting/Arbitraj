//! Файл: core/cash_order.rs.
//! Назначение: общий вход именованной биржевой покупки обычного и смешанного направления.
//! Потребители: trade_ops и исполнение обоих биржевых IOC смешанного направления.
//! Зависимости: родной торговый счёт адаптера, ledger.sqlite, prepared client ID.
//! Настройки: существующая конфигурация всех включённых расчётных площадок.
use super::{
    engine::{Core, rd},
    types::{Side, TradeRequest, TradeResult},
};
use crate::{
    gateway::{
        contract::{Venue, VenueKind},
        errors::{ErrorCode, GatewayError, GatewayResult},
        pending::PendingRef,
        settlement_intent as intent,
    },
    storage::{
        cex_funds::{self as funds, Admission, Bucket, Claim},
        cex_funds_original as original,
    },
};
use rust_decimal::Decimal;
use std::sync::Arc;
pub(crate) const OPENING: &str = "cash_buy_origin";
/// Добавляет привязку к уже существующей записи открытия сделки. В этой точке нет HTTP/SQL.
pub(crate) async fn opening(
    core: &Core,
    d: &mut super::deal::Deal,
    qty: Decimal,
    worst: Decimal,
    plan_spend: Decimal,
) {
    let Ok(v) = core.inner.deps.gw.get(&d.direction.buy_on).await else {
        return;
    };
    if !v.supports_cash_reservation() {
        return;
    }
    if let (Ok(owner), Ok(currency)) = (
        v.settlement_account(),
        super::settlement_market::ticker(core, &d.direction.buy_on),
    ) {
        let ticker = super::venue_ticker::ticker_on(core, &d.direction.asset, &d.direction.buy_on);
        let cap = v
            .cash_opening_cap(&ticker, &currency, qty, worst)
            .unwrap_or(plan_spend);
        d.plan[OPENING] = serde_json::json!(crate::storage::cex_funds_watch::Opening {
            cap,
            owner,
            currency
        });
    }
}
pub(crate) fn remember(core: &Core, d: &mut super::deal::Deal) {
    let priors = core
        .inner
        .deps
        .db
        .ledger()
        .cash_watch
        .finished_attempts(&d.direction.buy_on.0, &d.id);
    if !priors.is_empty() {
        d.plan["cash_legacy_ioc"] = serde_json::json!(priors);
    }
}
#[derive(Clone)]
struct Active {
    core: Core,
    venue: Arc<dyn Venue>,
    req: TradeRequest,
    key: String,
    currency: String,
    funding: bool,
    credentials: String,
    owned: Arc<std::sync::Mutex<Option<Owned>>>,
}
struct Owned {
    bucket: Bucket,
    client: String,
    marker: Option<(String, String)>,
}
tokio::task_local! {static ACTIVE: Active;}
struct Previous {
    record: Arc<crate::storage::cex_funds_watch::Legacy>,
    venue: Arc<dyn Venue>,
    request: TradeRequest,
    currency: Option<String>,
    opening: Option<crate::storage::cex_funds_watch::Opening>,
    key: String,
    db: crate::storage::db::Db,
}
tokio::task_local! {static LEGACY:Previous;}
pub(crate) async fn track_legacy(p: &PendingRef) -> GatewayResult<()> {
    let work = LEGACY
        .try_with(|x| {
            let cap = x.currency.as_ref().and_then(|c| {
                x.venue
                    .cash_order_cap(&x.request, p, c)
                    .ok()
                    .map(|n| (c.clone(), n))
            });
            let currency = x
                .currency
                .as_ref()
                .filter(|c| {
                    x.venue
                        .matches_settlement_order(p, &x.request.asset.ticker, c)
                })
                .cloned();
            let adjustment = x.opening.as_ref().is_some_and(|o| {
                cap.as_ref()
                    .is_none_or(|(c, n)| c != &o.currency || *n > o.cap)
            });
            x.record
                .prepare(p, currency, cap)
                .map_err(|e| err(ErrorCode::StaleData, &x.request, e))?;
            Ok::<_, GatewayError>(if adjustment {
                x.record
                    .snapshot(&x.key, false)
                    .map(|prior| (x.db.clone(), prior))
            } else {
                None
            })
        })
        .unwrap_or(Ok(None))?;
    if let Some((db, prior)) = work {
        cold::save_adjustment(&db, &prior)
            .await
            .map_err(|e| err(ErrorCode::Internal, &prior.request, e))?;
    }
    Ok(())
}
fn err(code: ErrorCode, req: &TradeRequest, s: impl Into<String>) -> GatewayError {
    GatewayError::new(code, Some(req.venue.clone()), s)
}
fn configured(core: &Core, req: &TradeRequest) -> bool {
    let dexes: Vec<_> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, kind)| *kind == VenueKind::Dex)
        .map(|(v, _)| v.clone())
        .collect();
    dexes
        .iter()
        .filter(|v| super::registry_sync::enabled(core, v))
        .any(|v| {
            crate::settings::evm_profiles::uses_settlement_contract(&core.inner.deps.settings, &v.0)
                && super::settlement_market::Market::new(core, v, &req.venue)
                    .is_ok_and(|m| m.cex_symbol() != m.quote)
        })
}
fn cash_key(req: &TradeRequest, attempt: u64, outer: Option<&str>) -> GatewayResult<String> {
    serde_json::to_string(&(
        "cash-buy",
        &req.deal_id,
        &req.venue,
        req.side,
        attempt,
        outer,
    ))
    .map_err(|e| err(ErrorCode::Internal, req, e.to_string()))
}
/// В старом режиме сам адаптер выполняет исходный ордер. При наличии общей валюты та же бронь учитывается
/// и для BSC/Arbitrum, даже если сам создающий её расчётный источник уже выключили.
pub async fn order(
    core: &Core,
    venue: Arc<dyn Venue>,
    req: TradeRequest,
    attempt: u64,
    funding: bool,
) -> GatewayResult<TradeResult> {
    if req.side != Side::Buy || !venue.supports_cash_reservation() {
        return venue.trade(req).await;
    }
    let db = core.inner.deps.db.ledger();
    let outer = intent::current_key();
    let key = cash_key(&req, attempt, outer.as_deref())?;
    let required = outer.is_some()
        || venue.supports_settlement_withdrawal()
            && venue.supports_settlement_credit()
            && configured(core, &req);
    let cred = venue.settlement_account()?;
    let lease = db
        .cash_watch
        .enter(&req.venue.0, &key, required, &req, &cred)
        .map_err(|e| err(ErrorCode::StaleData, &req, e))?;
    if let Some(prior) = &lease.legacy {
        let currency = super::settlement_market::ticker(core, &req.venue).ok();
        let opening = db.cash_watch.saved(&req.venue.0, &req.deal_id);
        let managed = super::engine::lk(&core.inner.deals)
            .get(&req.deal_id)
            .is_some_and(|d| d.plan.get(OPENING).is_some());
        if managed
            && opening
                .as_ref()
                .is_none_or(|o| o.owner != cred || Some(&o.currency) != currency.as_ref())
        {
            return Err(err(
                ErrorCode::NotConfigured,
                &req,
                "Первоначальный бюджет подписанта не подтверждён записью сделки",
            ));
        }
        let (result, gross) = LEGACY
            .scope(
                Previous {
                    record: prior.clone(),
                    venue: venue.clone(),
                    request: req.clone(),
                    currency,
                    opening,
                    key: key.clone(),
                    db: core.inner.deps.db.clone(),
                },
                crate::gateway::cash_signer::scope(
                    cred,
                    intent::capture_optional(venue.trade(req)),
                ),
            )
            .await;
        prior.result(match (&result, gross) {
            (Ok(trade), Some(gross_quote)) => Some(intent::SpotFill {
                trade: trade.clone(),
                gross_quote,
            }),
            _ => None,
        });
        return result;
    }
    let currency = super::settlement_market::ticker(core, &req.venue)
        .map_err(|e| err(ErrorCode::NotConfigured, &req, e))?;
    let mut candidates = cold::unadopted(db, &req.venue)
        .await
        .map_err(|e| err(ErrorCode::Internal, &req, e))?;
    for prior in &lease.prior {
        candidates.insert(prior.action.clone(), prior.clone());
    }
    let mut accounts = std::collections::HashMap::new();
    for prior in candidates.into_values() {
        let original_currency = prior
            .limit
            .as_ref()
            .map(|(c, _)| c)
            .or(prior.currency.as_ref());
        if prior.action != key && original_currency.is_some_and(|c| c != &currency) {
            continue;
        }
        cold::adopt(core, venue.as_ref(), &cred, &prior, &mut accounts).await?;
    }
    let old = if lease.known {
        original::load(db, &key)
            .await
            .map_err(|e| err(ErrorCode::Internal, &req, e))?
    } else {
        None
    };
    if lease.known && old.is_none() {
        return Err(err(
            ErrorCode::OutcomeUnknown,
            &req,
            "Сохранённый исходный ID покупки требует сверки",
        ));
    }
    let protected = crate::gateway::cash_signer::scope(cred.clone(), async {
        let fill = if let Some(o) = old {
            let bucket = venue.cash_identity(&o.currency).await?;
            if !original::binding(&o, &bucket)
                || req.deal_id != o.request.deal_id
                || req.asset != o.request.asset
                || req.asset.ticker != o.request.asset.ticker
                || cred != venue.settlement_account()?
            {
                return Err(err(
                    ErrorCode::OutcomeUnknown,
                    &req,
                    "Изменился исходный счёт, актив или ключ сохранённого IOC",
                ));
            }
            let prior = o.lookup.clone();
            intent::capture(async {
                venue
                    .inspect_settlement_order(o.request, &prior)
                    .await?
                    .ok_or_else(|| {
                        err(
                            ErrorCode::OutcomeUnknown,
                            &req,
                            "Статус исходного IOC ожидает сверки",
                        )
                        .pending(prior)
                    })
            })
            .await?
        } else {
            let active = Active {
                core: core.clone(),
                venue: venue.clone(),
                req: req.clone(),
                key: key.clone(),
                currency,
                funding,
                credentials: cred.clone(),
                owned: Default::default(),
            };
            ACTIVE
                .scope(active, intent::capture(venue.trade(req.clone())))
                .await?
        };
        let o = original::load(db, &key)
            .await
            .map_err(|e| err(ErrorCode::Internal, &req, e))?
            .ok_or_else(|| {
                err(
                    ErrorCode::OutcomeUnknown,
                    &req,
                    "Не найден исходный резерв после ответа биржи",
                )
            })?;
        finish(&core.inner.deps.db, &key, &req, &o, &fill).await?;
        intent::note_gross(Some(fill.gross_quote));
        Ok(fill.trade)
    })
    .await;
    match protected {
        Err(mut cause) if cause.code == ErrorCode::OutcomeUnknown => {
            if let Some(p) = &mut cause.pending {
                p.cash_action = Some(key);
            } else if let Ok(Some(o)) = original::load(db, &key).await {
                let mut p = o.lookup;
                p.cash_action = Some(key);
                cause = cause.pending(p);
            }
            Err(cause)
        }
        other => other,
    }
}
#[path = "cash_order_finish.rs"]
mod close;
#[path = "cash_order_cold.rs"]
mod cold;
use close::finish;
pub(crate) use close::{recovered, resolve};
#[path = "cash_order_prepare.rs"]
mod prepare;
pub(crate) use prepare::not_dispatched;
pub use prepare::{active, before_send};

#[cfg(test)]
#[path = "cash_order_tests.rs"]
mod tests;
