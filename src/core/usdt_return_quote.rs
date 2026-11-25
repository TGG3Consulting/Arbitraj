//! Комиссия, округление и газ возврата будущей выручки в расчёте ручной продажи.
use super::dirctx_venue::quote_network;
use super::engine::Core;
use crate::core::types::{CoinNetworkInfo, DexAccountId, VenueId};
use crate::gateway::contract::VenueKind;
use rust_decimal::Decimal;

// Return costs for proceeds that the proposed sale would produce. Balances and holds
// are checked by the actual transfer after the sale; the USDT need not exist yet.
pub(super) async fn cost(
    core: &Core,
    from: &VenueId,
    to: &VenueId,
    wallet: Option<&DexAccountId>,
    budget: Decimal,
) -> Result<Decimal, String> {
    if from == to {
        return Ok(Decimal::ZERO);
    }
    let usdt = super::usdt_return::route_asset(core, from, to)?;
    let net = super::transfers::network(core, &usdt, from)?;
    super::net_ready::withdraw(core, from, &usdt, &net)?;
    let (arrives, costs) = if super::trade_ops::kind(core, from)? == VenueKind::Cex {
        let info = quote_network(core, from, &usdt.ticker, &net)
            .ok_or_else(|| format!("{}: нет данных о выводе {} ({net})", from.0, usdt.ticker))?;
        super::dirctx_venue::fresh(core, &info)?;
        if !info.withdraw_enabled {
            return Err(format!("{}: вывод {} ({net}) закрыт", from.0, usdt.ticker));
        }
        (projected_cex_receipt(&info, budget)?, Vec::new())
    } else {
        let wallet = wallet.ok_or("не определён кошелёк возврата USDT")?;
        let adapter = core
            .inner
            .deps
            .gw
            .get_as(from, Some(wallet))
            .await
            .map_err(|e| e.message)?;
        let costs = adapter
            .withdraw_cost(&usdt, &net, budget)
            .await
            .map_err(|e| e.message)?;
        let paid: Decimal = costs
            .iter()
            .filter(|c| !c.in_price && c.asset == usdt.ticker)
            .map(|c| c.amount)
            .sum();
        (budget - paid, costs)
    };
    if arrives <= Decimal::ZERO {
        return Err(format!(
            "{}: выручка не покрывает комиссию возврата {}",
            from.0, usdt.ticker
        ));
    }
    if super::trade_ops::kind(core, to)? == VenueKind::Cex {
        let target = super::precheck_deposit::target_on(core, &usdt, to)?;
        let dest = quote_network(core, to, &target.on.ticker, &target.net).ok_or_else(|| {
            format!(
                "{}: нет данных о депозите {} ({})",
                to.0, target.on.ticker, target.net
            )
        })?;
        super::dirctx_venue::fresh(core, &dest)?;
        if !dest.deposit_enabled {
            return Err(format!(
                "{}: депозит {} ({}) закрыт",
                to.0, target.on.ticker, target.net
            ));
        }
        let min = super::usdt_transfer_check::deposit_min_known(
            &to.0,
            &target.on.ticker,
            &target.net,
            dest.deposit_min,
        )?;
        if arrives < min {
            return Err(format!(
                "{}: придёт {arrives} {}, минимум {min}",
                to.0, target.on.ticker
            ));
        }
    }
    let mut cost = budget - arrives;
    for charge in costs
        .iter()
        .filter(|c| !c.in_price && c.asset != usdt.ticker)
    {
        cost += super::fills::usdt(core, from, &usdt.ticker, Decimal::ONE, charge).await?;
    }
    Ok(cost)
}

fn projected_cex_receipt(info: &CoinNetworkInfo, budget: Decimal) -> Result<Decimal, String> {
    if let Some(amount) =
        super::withdraw_amount::for_network(budget, Some(info)).map_err(|e| e.to_string())?
    {
        return Ok(amount.wire);
    }
    let amount = super::costs_withdraw::floor_precision(budget, Some(info));
    let fee = super::costs_withdraw::cex_return_fee(info, amount).map_err(|e| e.to_string())?;
    Ok(amount - fee.amount)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::Asset;
    fn info(external: bool) -> CoinNetworkInfo {
        CoinNetworkInfo {
            venue: VenueId("cex".into()),
            asset: Asset {
                chain: None,
                contract: None,
                ticker: "USDT".into(),
            },
            network: "BSC".into(),
            deposit_enabled: true,
            withdraw_enabled: true,
            withdraw_fee: Some(Decimal::new(15, 1)),
            withdraw_fee_pct: None,
            use_net_withdrawal_budget: external,
            withdraw_min: Some(5.into()),
            withdraw_max: Some(10.into()),
            deposit_min: None,
            deposit_confirmations: None,
            withdraw_precision: Some(0),
            withdraw_step: None,
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn models_fixed_fee_rounding_and_limits_using_exchange_wire_convention() {
        let gross = info(false);
        let external = info(true);
        // Ordinary exchange rounds the gross down to 9, then deducts 1.5.
        assert_eq!(
            projected_cex_receipt(&gross, Decimal::new(99, 1)).unwrap(),
            Decimal::new(75, 1)
        );
        // HTX deducts the fee first, then rounds the on-chain amount down.
        assert_eq!(
            projected_cex_receipt(&external, Decimal::new(99, 1)).unwrap(),
            8.into()
        );
        assert!(projected_cex_receipt(&external, 6.into()).is_err());
        assert!(projected_cex_receipt(&gross, 13.into()).is_err());
        let unknown = CoinNetworkInfo {
            withdraw_fee: None,
            ..gross
        };
        assert!(projected_cex_receipt(&unknown, 9.into()).is_err());
    }
}
