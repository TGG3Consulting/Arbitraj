//! Compare the Gate tariff used for this request with the final fee for this very withdrawal.
use super::engine::{Core, wr};
use crate::core::types::{Asset, CostKind, Transfer, VenueId, WithdrawRequest};
use crate::storage::journal::Level;
use rust_decimal::Decimal;
use std::sync::Arc;

pub(super) fn expected(
    core: &Core,
    from: &VenueId,
    asset: &Asset,
    net: &str,
    qty: Decimal,
) -> Option<Decimal> {
    if from.0 != "gate" {
        return None;
    }
    super::dirctx_venue::network(core, from, asset, net)?.fee_for(qty)
}

fn charged(req: &WithdrawRequest, id: &str, sent: &Transfer) -> Option<Decimal> {
    if id.is_empty()
        || sent.venue_ref != id
        || sent.venue != req.venue
        || sent.asset.ticker != req.asset.ticker
        || sent.network != req.network
    {
        return None;
    }
    let fee = sent.fee.as_ref()?;
    (fee.kind == CostKind::WithdrawFee
        && fee.asset == req.asset.ticker
        && !fee.in_price
        && fee.paid_in.as_ref().is_none_or(|p| p == &fee.asset))
    .then_some(fee.amount)
}

pub(super) fn finished(
    core: &Core,
    req: &WithdrawRequest,
    id: &str,
    sent: &Transfer,
    expected: Option<Decimal>,
) {
    let Some(announced) = expected else { return };
    let Some(actual) = charged(req, id, sent) else {
        return;
    };
    if announced == actual {
        return;
    }
    let core = core.clone();
    let req = req.clone();
    let id = id.to_owned();
    let scope = (
        req.venue.clone(),
        req.asset.ticker.clone(),
        req.network.clone(),
    );
    let refresh = core
        .inner
        .withdraw_fee_refresh
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(scope.clone());
    tokio::spawn(async move {
        let msg = format!(
            "{}: вывод {id}, {} в сети {}: объявлено {announced} {}, удержано {actual} {}",
            req.deal_id, req.amount, req.network, req.asset.ticker, req.asset.ticker
        );
        core.inner
            .deps
            .log(
                Level::Warn,
                "core.withdraw_fee",
                Some(&req.venue.0),
                msg,
                None,
            )
            .await;
        if !refresh {
            return;
        }
        let result = refresh_fee(&core, &req).await;
        core.inner
            .withdraw_fee_refresh
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&scope);
        if let Err(e) = result {
            core.inner
                .deps
                .log(
                    Level::Warn,
                    "core.withdraw_fee",
                    Some(&req.venue.0),
                    e,
                    None,
                )
                .await;
        }
    });
}

async fn refresh_fee(core: &Core, req: &WithdrawRequest) -> Result<(), String> {
    // Capture the old tariff: a newer directory update that finished during this HTTP request wins.
    let Some(old) =
        super::dirctx_venue::quote_network(core, &req.venue, &req.asset.ticker, &req.network)
    else {
        return Ok(());
    };
    let gw = core
        .inner
        .deps
        .gw
        .get(&req.venue)
        .await
        .map_err(|e| e.message)?;
    let rows = gw
        .refresh_withdraw_tariff(&req.asset.ticker, &req.network)
        .await
        .map_err(|e| e.message)?;
    let rows = super::net_chain::prepare(core, &req.venue, rows);
    let Some(new) = rows.into_iter().find(|x| {
        x.network == old.network && x.asset == old.asset && x.asset.ticker == old.asset.ticker
    }) else {
        return Err(format!(
            "При обновлении тарифа {} нет совпадающего актива и сети {}",
            req.asset.ticker, req.network
        ));
    };
    if new.withdraw_fee == old.withdraw_fee && new.withdraw_fee_pct == old.withdraw_fee_pct {
        return Ok(());
    }
    let updated = {
        let mut nets = wr(&core.inner.networks);
        if let Some(list) = nets.get_mut(&req.venue)
            && let Some(index) = list.iter().position(|x| {
                x.network == old.network
                    && x.asset == old.asset
                    && x.asset.ticker == old.asset.ticker
                    && x.withdraw_fee == old.withdraw_fee
                    && x.withdraw_fee_pct == old.withdraw_fee_pct
            })
        {
            let row = &mut Arc::make_mut(list)[index];
            row.withdraw_fee = new.withdraw_fee;
            row.withdraw_fee_pct = new.withdraw_fee_pct;
            true
        } else {
            false
        }
    };
    if updated {
        core.request_rebuild();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Cost, TransferStatus};
    use chrono::Utc;
    #[test]
    fn gate_ai_final_40_compares_to_announced_36_34_only_on_the_same_withdrawal() {
        let asset = Asset {
            ticker: "AI".into(),
            chain: None,
            contract: None,
        };
        let req = WithdrawRequest {
            venue: VenueId("gate".into()),
            asset: asset.clone(),
            network: "BSC".into(),
            address: "address".into(),
            amount: Decimal::from(1000),
            deal_id: "20261122120942578AI".into(),
        };
        let mut result = Transfer {
            venue: req.venue.clone(),
            asset,
            network: "BSC".into(),
            amount: req.amount,
            venue_ref: "w17".into(),
            tx_hash: None,
            status: TransferStatus::Sent,
            updated_at: Utc::now(),
            fee: Some(Cost::new(CostKind::WithdrawFee, 40.into(), "AI", false)),
        };
        let planned = Decimal::new(3634, 2);
        assert_eq!(
            charged(&req, "w17", &result).unwrap() - planned,
            Decimal::new(366, 2)
        );
        assert_eq!(charged(&req, "w18", &result), None);
        result.network = "ARB".into();
        assert_eq!(charged(&req, "w17", &result), None);
        result.network = "BSC".into();
        result.fee.as_mut().unwrap().asset = "GT".into();
        assert_eq!(charged(&req, "w17", &result), None);
    }
}
