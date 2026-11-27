//! Сравнение исполнений одного объёма с фактическим газом и разрешениями.
use super::{Quoting, pick};
use crate::core::types::{Quote, Side};
use crate::gateway::quote_stage;
use rust_decimal::Decimal;
pub(super) type OptionQuote = (Quote, Option<(u64, usize)>);
pub(super) async fn choose(q: &Quoting, baseline: Quote, alternate: Quote, dec: u8) -> Quote {
    let ok = |x: &Quote| {
        x.route.as_ref().is_some_and(|r| r.executable)
            && (!quote_stage::settlement()
                || quote_stage::settlement_quote_supported(x, quote_stage::is_final()))
    };
    if !ok(&alternate) { return baseline; }
    if !ok(&baseline) || alternate.fillable_qty > baseline.fillable_qty {
        return alternate;
    }
    if alternate.fillable_qty < baseline.fillable_qty {
        return baseline;
    }
    let (Some(a), Some(b)) = (
        pick::gas_usdt(q, &baseline, dec).await,
        pick::gas_usdt(q, &alternate, dec).await,
    ) else {
        return baseline;
    };
    if prefer(
        alternate.side,
        (baseline.avg_price * baseline.fillable_qty, a),
        (alternate.avg_price * alternate.fillable_qty, b),
    ) {
        alternate
    } else {
        baseline
    }
}

fn prefer(
    side: Side,
    (gross_a, gas_a): (Decimal, Decimal),
    (gross_b, gas_b): (Decimal, Decimal),
) -> bool {
    match side {
        Side::Buy => {
            gross_b + gas_b < gross_a + gas_a
                || (gross_b + gas_b == gross_a + gas_a && gross_b < gross_a)
        }
        Side::Sell => {
            gross_b - gas_b > gross_a - gas_a
                || (gross_b - gas_b == gross_a - gas_a && gas_b < gas_a)
        }
    }
}

pub(super) struct Execution {
    pub mode: crate::core::types::RouteMode,
    pub target: alloy_primitives::Address,
    pub wire: super::v2_wallet_sim::Wire,
}
pub(super) async fn option(
    q: &Quoting,
    i: &super::routing::Input,
    e: super::super::pools::registry::Entry,
    payer: alloy_primitives::Address,
    mut execution: Execution,
    h: Option<super::white_state::Head>,
) -> crate::gateway::errors::GatewayResult<OptionQuote> {
    use super::super::{exec::executor, gas_ticker, num};
    use super::{
        costs, direct,
        path::Path,
        steps::Names,
        v2_wallet_sim::{self, Order},
    };
    use crate::core::types::RouterFeeMode;
    use crate::{core::types::RouteMode, gateway::errors::ErrorCode};
    use alloy_primitives::U256;
    let ctx = &q.ctx;
    let decimals = (i.coin_dec, i.usdt_dec);
    let (mut c, _) = direct::best(q, &i.asset, i.side, i.qty, vec![e], decimals.0, decimals.1)?;
    if let crate::core::types::PoolState::Antfarm(book)=&c.entry.pool.state{
        let (input,output)=if i.side==Side::Buy{(i.usdt,i.coin)}else{(i.coin,i.usdt)};
        if let Some((tier,funds))=book.proceeds {
            let fill=book.output_order(input,true,c.swapped.amount_in).ok_or_else(||ctx.err(ErrorCode::NoRoute,"Antfarm: комиссия не покрыта выплатой"))?;
            let loan=matches!(funds,crate::core::types::AntfarmFunds::Vault{..});
            let terms=super::super::exec::antfarm_output_wallet::Terms::new((execution.target,book.market.atf),input,output,book.market.tier,tier,loan,&fill);
            execution.wire=super::v2_wallet_sim::Wire::AntfarmOutput(std::sync::Arc::new(terms));
        } else {
        let terms=super::super::exec::antfarm_wallet::Terms::from_budget(book,execution.target,input,output,c.swapped.amount_in)
            .ok_or_else(||ctx.err(ErrorCode::NoRoute,"Antfarm: объём и комиссия не помещаются в указанный бюджет"))?;
        execution.wire=super::v2_wallet_sim::Wire::Antfarm(std::sync::Arc::new(terms));
        }
    }
    let fee_mode = match &execution.wire {
        super::v2_wallet_sim::Wire::RegisteredV3(_) | super::v2_wallet_sim::Wire::SolidlyRouter(_) => Some(RouterFeeMode::ProtectedWallet{target:execution.target}),
        super::v2_wallet_sim::Wire::Umbrae(_) | super::v2_wallet_sim::Wire::DirectModule{..} if c.entry.family.variant.umbrae() || c.entry.family.variant==super::super::pools::family_variant::Variant::VvsV3=>Some(RouterFeeMode::ProtectedWallet{target:execution.target}),
        super::v2_wallet_sim::Wire::O1(_)=>Some(RouterFeeMode::ProtectedWallet{target:execution.target}),
        super::v2_wallet_sim::Wire::AntfarmOutput(t)=>Some(RouterFeeMode::AntfarmProceeds{target:t.module,tier:t.funding,loan:t.use_loan}),
        super::v2_wallet_sim::Wire::Antfarm(t)=>Some(RouterFeeMode::ProtectedWallet{target:t.module}),
        super::v2_wallet_sim::Wire::Temple(k)=>Some(RouterFeeMode::ProtectedWallet{target:k.module}),
        super::v2_wallet_sim::Wire::Carbon(c)=>Some(RouterFeeMode::CarbonWallet{module:c.module,allocation:Some(c.allocation()),native_cash:c.wrapped().is_some() && super::vista_pricing::cash(&q.ctx,&i.usdt_ticker,i.usdt)}),
        super::v2_wallet_sim::Wire::MintClub(c) if c.protection.is_some()=>c.protection.map(|p|RouterFeeMode::ProtectedWallet{target:p.module}),
        super::v2_wallet_sim::Wire::EkuboV2(_,Some(_)) if super::vista_pricing::cash(&q.ctx,&i.usdt_ticker,i.usdt)=>Some(RouterFeeMode::NativeSettlement{gross_minimum:false}),
        super::v2_wallet_sim::Wire::Vista(v)
            if super::vista_pricing::cash(&q.ctx, &i.usdt_ticker, i.usdt) =>
        {
            Some(RouterFeeMode::NativeSettlement {
                gross_minimum: v.form == super::super::exec::vista_wire::Form::OfficialEthOut,
            })
        }
        super::v2_wallet_sim::Wire::Vista(v)
            if v.form == super::super::exec::vista_wire::Form::OfficialEthOut =>
        {
            Some(RouterFeeMode::NativeGrossMinimum)
        }
        super::v2_wallet_sim::Wire::Oasis { rebate: true } => Some(RouterFeeMode::Rebate),
        super::v2_wallet_sim::Wire::Oasis { rebate: false } => Some(RouterFeeMode::Standard),
        _ => None,
    };
    let mode = execution.mode;
    let target = execution.target;
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Не подтверждены полнота объёма или способ исполнения V2",
        )
    };
    if c.coins != i.qty {
        return Err(bad());
    }
    let carbon_fee=if let super::v2_wallet_sim::Wire::Carbon(order)=&execution.wire{
        let (spent,received,fee)=order.estimate.ok_or_else(bad)?;
        c.swapped.amount_in=U256::from(spent);
        c.swapped.amount_out=U256::from(received);
        let buy=i.side==Side::Buy;
        c.usdt=num::to_decimal(U256::from(if buy{spent}else{received}),i.usdt_dec).ok_or_else(bad)?;
        let decimals=if order.by_target{if buy{i.usdt_dec}else{i.coin_dec}}else if buy{i.coin_dec}else{i.usdt_dec};
        Some(num::to_decimal(U256::from(fee),decimals).ok_or_else(bad)?)
    }else{None};
    let gas_t = gas_ticker::read(ctx)?;
    let names = Names {
        usdt: (i.usdt, &i.usdt_ticker),
        coin: (i.coin, &i.asset.ticker),
        mids: &q.mids,
        gas: &gas_t,
    };
    let build = |c| {
        costs::quote(
            ctx,
            &i.asset,
            i.side,
            i.qty,
            &[Path::one(ctx, c, i.side, decimals)?],
            mode,
            &i.taxes,
            i.gas,
            &names,
        )
    };
    if mode == RouteMode::Graph {
        let provisional = build(c.clone())?;
        let route = provisional.route.as_ref().ok_or_else(bad)?;
        if executor::takes(
            ctx,
            &q.reg,
            route,
            super::super::exec::native::is_native(ctx, &i.asset),
        )? != Some(target)
        {
            return Err(bad());
        }
    }
    let mut approval = None;
    if let Some(head) = h {
        let (input, output, amount, minimum) = match i.side {
            Side::Buy => (
                i.usdt,
                i.coin,
                c.swapped.amount_in,
                direct::ceil_units(i.qty, i.coin_dec).ok_or_else(bad)?,
            ),
            Side::Sell => (
                i.coin,
                i.usdt,
                num::to_units(i.qty, i.coin_dec).ok_or_else(bad)?,
                U256::from(1),
            ),
        };
        if matches!(&execution.wire, super::v2_wallet_sim::Wire::Router02)
            && super::router_pricing::is_native(&c.entry)
        {
            super::router_pricing::checked(ctx, &c.entry, input, amount, head).await?;
        }
        let vista_value = if let super::v2_wallet_sim::Wire::Carbon(c)=&execution.wire{Some(c.value())}else if let super::v2_wallet_sim::Wire::EkuboV2(key,Some(_))=&execution.wire {Some(if key.side{U256::ZERO}else{amount})}else if super::vista_pricing::is_vista(&c.entry) || matches!(&execution.wire,super::v2_wallet_sim::Wire::Vista(_)) {
            Some(match &execution.wire {
                super::v2_wallet_sim::Wire::Vista(call) => call.value(amount),
                super::v2_wallet_sim::Wire::V5(v) if matches!(v.native, Some((_, true))) => amount,
                _ => U256::ZERO,
            })
        } else {
            None
        };
        let o = Order {
            payer,
            target,
            input,
            output,
            amount,
            minimum,
            wire: execution.wire,
        };
        let fill = v2_wallet_sim::measure(
            ctx,
            head,
            o,
            super::oasis_pricing::is_oasis(&c.entry) || super::vista_pricing::is_vista(&c.entry)
                || super::super::exec::vault_direct::enabled(&c.entry) || super::module_pricing::enabled(&c.entry) || vista_value.is_some() || c.entry.family.variant.exact_router() || super::ekubo_v2_pricing::enabled(&c.entry) || super::super::exec::mint_club_direct::enabled(&c.entry),
        )
        .await?;
        if let Some(value) = vista_value {
            Box::pin(super::vista_cash::funded(
                q, head, payer, value, &fill, i.gas,
            ))
            .await?;
        }
        if i.side == Side::Sell {
            c.usdt = num::to_decimal(fill.received, i.usdt_dec).ok_or_else(bad)?;
        }
        c.usable.gas = fill.swap_gas;
        approval = Some(fill.approval);
    }
    let mut quote = build(c)?;
    if let Some(fee)=carbon_fee && let Some(row)=quote.costs.iter_mut().find(|c|c.kind==crate::core::types::CostKind::PoolFee){row.amount=fee;}
    if fee_mode.is_some() {
        let route = quote.route.as_mut().ok_or_else(bad)?;
        let [leg] = route.legs.as_mut_slice() else {
            return Err(bad());
        };
        let [hop] = leg.hops.as_mut_slice() else {
            return Err(bad());
        };
        hop.fee_mode = fee_mode;
    }
    Ok((quote, approval))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn funded_router_and_v5_include_wallet_delta_rollback_and_separate_approval_cost() {
        let raw: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/whiteswap_settlement.json"
        ))
        .unwrap();
        let rows = raw["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 36);
        let value = |v: &Value| v.as_str().unwrap().parse::<u128>().unwrap();
        for row in rows {
            assert_eq!(row["requested"], row["spent"]);
            assert_eq!(
                value(&row["before"][0]) - value(&row["after"][0]),
                value(&row["spent"])
            );
            assert_eq!(
                value(&row["after"][1]) - value(&row["before"][1]),
                value(&row["received"])
            );
            assert_eq!(row["rollback"], row["before"]);
            assert_eq!(row["min_out"], row["received"]);
            assert!(row["rejected"].is_object());
        }
        let sell = |mode: &str| {
            rows.iter()
                .find(|r| r["mode"] == mode && r["tier"] == 2 && r["requested"] == "1713590")
                .unwrap()
        };
        let router = sell("router_net");
        let v5 = sell("v5");
        assert_eq!(router["received"], "994407");
        assert_eq!(v5["received"], "994207");
        let r = Decimal::from(994407u64);
        let v = Decimal::from(994207u64);
        let gas = |row: &Value| Decimal::from(row["swap_gas"].as_u64().unwrap());
        // При 1 base unit USDT / gas и одинаковых разрешениях Native выигрывает.
        assert!(prefer(Side::Sell, (v, gas(v5)), (r, gas(router))));
        // Тот же рынок и размер, у V5 сохранено разрешение; у роутера ещё нужен approve.
        let rg = gas(router) + Decimal::from(router["approval_gas"][0].as_u64().unwrap());
        assert!(!prefer(Side::Sell, (v, gas(v5)), (r, rg)));
    }
}
