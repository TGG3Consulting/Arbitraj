//! Сравнение доступных заявок AirSwap с исполнимой котировкой маршрута.
pub(in crate::venues::bsc_dex) use super::book::available;
pub(super) use super::book::{Ticket, within_input};
use super::{Limits, enabled};
use crate::{
    core::types::{Asset, Quote, Side},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        quote_stage,
    },
    venues::bsc_dex::{
        num,
        parts::Parts,
        quote::{rfq_feed, white_state},
        wallet,
    },
};
use alloy_primitives::U256;
use rust_decimal::Decimal;
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    a: &Asset,
    s: Side,
    qty: Decimal,
    old: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    if !available(p)
        || quote_stage::settlement()
        || qty <= Decimal::ZERO
        || old.as_ref().is_err_and(|e| e.code != ErrorCode::NoRoute)
    {
        return old;
    }
    let Some(pair) = p.airswap.pair(p, a, s) else {
        return old;
    };
    let c = &p.ctx;
    let Ok(who) = wallet::signer::address(c) else {
        return old;
    };
    let buy = s == Side::Buy;
    let mut candidates = p.airswap.candidates(p, a, s).unwrap_or_default();
    candidates.retain(|t| {
        let (v, d) = if buy {
            (t.received, t.dout)
        } else {
            (t.spent, t.di)
        };
        num::to_decimal(v, d) == Some(qty)
    });
    if candidates.is_empty() {
        let Ok(timeout) = c.timeout() else { return old };
        let discover = async {
            let l = super::indicative::observe(p, s, qty, pair, old.is_err()).await?;
            let id = rfq_feed::identity(c)?;
            let key = ((id, pair.0, pair.1), who);
            let ms = c.int("airswap.min_validity_ms").ok()?;
            for offer in &l.offers {
                let Some(price) =
                    super::indicative::priced(p, a, s, qty, &l, offer, old.as_ref().ok()).await
                else {
                    continue;
                };
                let max = if buy {
                    match quote_stage::buy_budget(c.place.id, pair.0) {
                        Some(v) => num::to_units(v, l.decimals.0)?,
                        None => offer.paid,
                    }
                } else {
                    l.query.amount
                };
                if offer.paid > max {
                    continue;
                }
                if !quote_stage::is_final() {
                    if old
                        .as_ref()
                        .is_ok_and(|v| v.route.as_ref().is_some_and(|r| r.executable))
                    {
                        return None;
                    }
                    return Some(Some(price));
                }
                let h = white_state::head(c, "latest").await.ok()?;
                let limits = Limits {
                    minimum: if buy { l.query.amount } else { U256::ONE },
                    maximum: max,
                };
                for scope in super::profile::Profile::candidates(c) {
                    let Some(prepared) = super::signed::prepare(
                        c,
                        l.query,
                        scope,
                        &offer.maker,
                        who,
                        h,
                        Some(offer.paid),
                    )
                    .await
                    .ok()
                    .flatten() else {
                        continue;
                    };
                    if c.settings.generation() != id.1
                        || crate::secrets::vault::generation() != id.2
                        || !enabled(c)
                    {
                        return None;
                    }
                    if !p.airswap.reservations.acquire(
                        key,
                        tokio::time::Instant::now(),
                        timeout.max(std::time::Duration::from_millis(ms)),
                    ) {
                        return None;
                    }
                    let rows = super::signed::signed(
                        c,
                        l.query,
                        limits,
                        h,
                        l.decimals,
                        scope,
                        &offer.maker,
                        (id.1, who, ms),
                        prepared,
                    )
                    .await
                    .ok()?;
                    for t in rows {
                        p.airswap.reservations.hold(key, t.proof.expiry);
                        p.airswap.put(t)
                    }
                    return Some(None);
                }
            }
            None
        };
        match tokio::time::timeout(timeout, Box::pin(discover)).await {
            Ok(Some(Some(q))) => return Ok(q),
            Ok(Some(None)) => candidates = p.airswap.candidates(p, a, s).unwrap_or_default(),
            _ => return old,
        }
    }
    super::ranking::best(p, a, s, qty, pair.0, old, candidates).await
}
