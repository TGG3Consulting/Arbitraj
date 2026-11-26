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
pub(crate) async fn opening(core: &Core, d: &mut super::deal::Deal, plan: &super::costs::Calc) {
    if let Ok(v) = core.inner.deps.gw.get(&d.direction.buy_on).await
        && v.supports_cash_reservation()
    {
        let bound = plan.size.as_ref().map(|s| s.budget);
        if let (Some(cap), Ok(owner), Ok(currency)) = (
            bound,
            v.settlement_account(),
            super::settlement_market::ticker(core, &d.direction.buy_on),
        ) {
            d.plan[OPENING] = serde_json::json!(crate::storage::cex_funds_watch::Opening {
                cap,
                owner,
                currency
            });
        }
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
}
tokio::task_local! {static ACTIVE: Active;}
tokio::task_local! {static LEGACY: (Arc<crate::storage::cex_funds_watch::Legacy>,Arc<dyn Venue>,TradeRequest,Option<String>,Option<crate::storage::cex_funds_watch::Opening>);}
pub(crate) fn track_legacy(p: &PendingRef) -> GatewayResult<()> {
    LEGACY
        .try_with(|(r, v, q, c, opening)| {
            let cap = c
                .as_ref()
                .and_then(|c| v.cash_order_cap(q, p, c).ok().map(|n| (c.clone(), n)));
            if let Some(opening) = opening
                && cap
                    .as_ref()
                    .is_none_or(|(c, n)| c != &opening.currency || *n > opening.cap)
            {
                return Err(
                    "Предел подготовленной покупки превышает сохранённый первоначальный бюджет"
                        .into(),
                );
            }
            r.prepare(p, cap)
        })
        .unwrap_or(Ok(()))
        .map_err(|e| GatewayError::new(ErrorCode::StaleData, None, e))
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
                (prior.clone(), venue.clone(), req.clone(), currency, opening),
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
    let mut candidates = cold::unadopted(db, &req.venue)
        .await
        .map_err(|e| err(ErrorCode::Internal, &req, e))?;
    for prior in &lease.prior {
        candidates.insert(prior.action.clone(), prior.clone());
    }
    let mut accounts = std::collections::HashMap::new();
    for prior in candidates.into_values() {
        cold::adopt(core, venue.as_ref(), &cred, &prior, &mut accounts).await?;
    }
    let old = if lease.known {
        original::load(db, &key)
            .await
            .map_err(|e| err(ErrorCode::Internal, &req, e))?
    } else {
        None
    };
    let currency = super::settlement_market::ticker(core, &req.venue)
        .map_err(|e| err(ErrorCode::NotConfigured, &req, e))?;
    crate::gateway::cash_signer::scope(cred.clone(), async {
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
    .await
}
#[path = "cash_order_finish.rs"]
mod close;
#[path = "cash_order_cold.rs"]
mod cold;
use close::finish;
pub(crate) use close::recovered;
#[path = "cash_order_prepare.rs"]
mod prepare;
pub use prepare::{active, before_send};

#[cfg(test)]
#[path = "cash_order_tests.rs"]
mod tests;
