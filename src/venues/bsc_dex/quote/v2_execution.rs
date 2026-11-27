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
    let measured=if let Some(head)=h && e.family.variant.aqua(){Some(Box::pin(super::aqua_sizing::paid(q,i,&e,payer,&execution,head)).await?)}
    else if let Some(head)=h && e.family.variant==super::super::pools::family_variant::Variant::Ambient{Some(Box::pin(super::ambient_pricing::paid(q,i,&e,payer,&execution,head)).await?)}
    else if let Some(head)=h && e.family.variant.llamma(){Some(Box::pin(super::llamma_pricing::paid(q,i,&e,payer,&execution,head)).await?)}
    else if let Some(head)=h && matches!(&e.pool.state,crate::core::types::PoolState::YieldBasisPool(_)|crate::core::types::PoolState::BancorLegacy(_)){
        let fill=Box::pin(super::yield_basis_sizing::paid(q,i,&e,payer,&execution,head)).await?;
        Some(fill)
    }else{None};
    let buy=measured.as_ref().map(|f|direct::BuyWire::Measured{spent:f.charged,received:f.received}).unwrap_or(match &execution.wire{
        v2_wallet_sim::Wire::BalancerV1(t) if t.desired.is_some()=>direct::BuyWire::BalancerTarget,
        v2_wallet_sim::Wire::BancorV3(t) if t.desired.is_some()=>direct::BuyWire::BancorTarget,
        _=>direct::BuyWire::Forward,
    });
    let (mut c, _) = direct::best_with(q,&i.asset,i.side,i.qty,vec![e],decimals,buy)?;
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
    // A plain SURGE sale checks gross ETH before a mutable tax. Do not let a cheap
    // quote win when even the configured minimum cannot be signed on this form.
    if matches!(&execution.wire,v2_wallet_sim::Wire::Vista(v) if v.form==super::super::exec::vista_wire::Form::SurgeEthOut) {
        let crate::core::types::PoolState::Surge(state)=&c.entry.pool.state else {
            return Err(ctx.err(ErrorCode::NoRoute,"Surge: отсутствуют данные для нативной продажи"));
        };
        let decimals=if i.side==Side::Buy{i.coin_dec}else{i.usdt_dec};
        let receive=num::to_decimal(c.swapped.amount_out,decimals);
        let slip=Decimal::ONE-ctx.dec("exec.dex_slippage_pct")?/Decimal::ONE_HUNDRED;
        let minimum=receive.and_then(|v|v.checked_mul(slip)).and_then(|v|num::to_units(v,decimals));
        let required=minimum.and_then(|v|super::super::math::surge::gross_for_net(v.max(U256::ONE),90));
        if !required.zip(state.gross(state.token,c.swapped.amount_in)).is_some_and(|(floor,gross)|floor<=gross) {
            return Err(ctx.err(ErrorCode::NoRoute,"Surge: нативная продажа не обеспечивает требуемый минимум после налога"));
        }
    }
    let fee_mode = match &execution.wire {
        v2_wallet_sim::Wire::V5Funded(step,_) if c.entry.family.variant.euler() && step.kind==17=>
            Some(RouterFeeMode::EulerModuleV5{module:step.pool,executor:execution.target}),
        v2_wallet_sim::Wire::V5Funded(step,_) if step.native.is_some() && super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt)=>
            Some(RouterFeeMode::NativeWallet{target:execution.target,native_cash:true}),
        super::v2_wallet_sim::Wire::Ranges(_) | super::v2_wallet_sim::Wire::RegisteredV3(_) | super::v2_wallet_sim::Wire::SolidlyRouter(_) => Some(RouterFeeMode::ProtectedWallet{target:execution.target}),
        super::v2_wallet_sim::Wire::Umbrae(_) | super::v2_wallet_sim::Wire::DirectModule{..} if c.entry.family.variant.ammalgam() || c.entry.family.variant.umbrae() || c.entry.family.variant.separate_v3_router()=>Some(RouterFeeMode::ProtectedWallet{target:execution.target}),
        super::v2_wallet_sim::Wire::FluxEth(m)=>Some(RouterFeeMode::FluxEth{target:m.router,native_cash:super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt)}),
        super::v2_wallet_sim::Wire::SafeSwap(_)|super::v2_wallet_sim::Wire::DegenRouter(_)|super::v2_wallet_sim::Wire::DegenModule(_)|super::v2_wallet_sim::Wire::Pendle(_)|super::v2_wallet_sim::Wire::O1(_)=>Some(RouterFeeMode::ProtectedWallet{target:execution.target}),
        super::v2_wallet_sim::Wire::AntfarmOutput(t)=>Some(RouterFeeMode::AntfarmProceeds{target:t.module,tier:t.funding,loan:t.use_loan}),
        super::v2_wallet_sim::Wire::Antfarm(t)=>Some(RouterFeeMode::ProtectedWallet{target:t.module}),
        super::v2_wallet_sim::Wire::Bundles(t)=>Some(RouterFeeMode::ProtectedWallet{target:t.target}),
        super::v2_wallet_sim::Wire::BancorLegacy(t)=>Some(RouterFeeMode::BancorLegacy{target:t.target,native:t.native,native_cash:t.native && super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt),principal:measured.as_ref().map(|f|f.charged),v5:None}),
        super::v2_wallet_sim::Wire::YieldBasis(t)=>Some(RouterFeeMode::YieldBasis{target:t.target,native:t.native,native_cash:t.native && super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt),principal:measured.as_ref().map(|f|f.charged),v5:None}),
        super::v2_wallet_sim::Wire::V5(step) if measured.is_some() && c.entry.family.variant==super::super::pools::family_variant::Variant::BancorLegacy =>
            Some(RouterFeeMode::BancorLegacy{target:step.pool,native:false,native_cash:false,principal:measured.as_ref().map(|f|f.charged),v5:Some(execution.target)}),
        super::v2_wallet_sim::Wire::V5(step) if measured.is_some() && c.entry.family.variant==super::super::pools::family_variant::Variant::YieldBasis =>
            Some(RouterFeeMode::YieldBasis{target:step.pool,native:false,native_cash:false,principal:measured.as_ref().map(|f|f.charged),v5:Some(execution.target)}),
        super::v2_wallet_sim::Wire::Mooniswap(t)=>Some(RouterFeeMode::Mooniswap{target:t.target,native:t.native,native_cash:t.native && super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt)}),
        super::v2_wallet_sim::Wire::DefiPlaza(t)=>Some(RouterFeeMode::DefiPlaza{target:t.target,native:t.native,native_cash:t.native && super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt)}),
        super::v2_wallet_sim::Wire::BalancerV1(t)=>Some(RouterFeeMode::BalancerV1{
            target:t.target,native:t.native,native_cash:t.native && super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt),desired:t.desired}),
        super::v2_wallet_sim::Wire::BancorV3(t)=>Some(RouterFeeMode::BancorV3{target:t.contract,module:t.module_hash.is_some(),exempt:t.exemption,desired:t.desired,native:t.native_wallet,native_cash:t.native_wallet && super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt)}),
        super::v2_wallet_sim::Wire::UniswapV1(t)=>Some(RouterFeeMode::UniswapV1{target:t.target,module:t.module_hash.is_some(),native:t.native().is_some(),native_cash:t.native().is_some() && super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt)}),
        super::v2_wallet_sim::Wire::Llamma(t)=>Some(RouterFeeMode::Llamma{target:t.target,desired:t.desired,maximum:c.swapped.amount_in,v5:None}),
        super::v2_wallet_sim::Wire::LlammaV5(step,_)=>Some(RouterFeeMode::Llamma{target:step.pool,desired:None,maximum:c.swapped.amount_in,v5:Some(execution.target)}),
        super::v2_wallet_sim::Wire::Rubicon(t)=>Some(RouterFeeMode::WalletBounds{target:t.target,desired:t.desired}),
        super::v2_wallet_sim::Wire::NativeFlat(t)=>Some(RouterFeeMode::NativeFlat{target:t.router,wei:t.wei}),
        super::v2_wallet_sim::Wire::Prop(t)=>Some(RouterFeeMode::VenueFallback{target:t.target,allowed:t.fallback_allowed}),
        super::v2_wallet_sim::Wire::Aktionariat(m)=>Some(RouterFeeMode::ProtectedWallet{target:m.module}),
        super::v2_wallet_sim::Wire::Aqua(t)=>Some(RouterFeeMode::ProtectedWallet{target:t.target}),
        super::v2_wallet_sim::Wire::Angstrom(t)=>Some(RouterFeeMode::ProtectedWallet{target:t.module}),
        super::v2_wallet_sim::Wire::Ambient(t)=>Some(RouterFeeMode::WalletBounds{target:t.target,desired:t.desired}),
        super::v2_wallet_sim::Wire::Stabull(t)=>Some(RouterFeeMode::WalletBounds{target:t.target,desired:t.desired}),
        super::v2_wallet_sim::Wire::Integral(t)=>Some(RouterFeeMode::WalletBounds{target:t.target,desired:t.desired}),
        super::v2_wallet_sim::Wire::Dagrow(t)=>Some(RouterFeeMode::Dagrow{target:t.target,desired:t.desired}),
        super::v2_wallet_sim::Wire::Temple(k)=>Some(RouterFeeMode::ProtectedWallet{target:k.module}),
        super::v2_wallet_sim::Wire::Carbon(c)=>Some(RouterFeeMode::CarbonWallet{module:c.module,allocation:Some(c.allocation()),native_cash:c.wrapped().is_some() && super::vista_pricing::cash(&q.ctx,&i.usdt_ticker,i.usdt)}),
        super::v2_wallet_sim::Wire::MintClub(c) if c.protection.is_some()=>c.protection.map(|p|RouterFeeMode::ProtectedWallet{target:p.module}),
        super::v2_wallet_sim::Wire::EkuboV2(_,Some(_)) if super::vista_pricing::cash(&q.ctx,&i.usdt_ticker,i.usdt)=>Some(RouterFeeMode::NativeSettlement{gross_minimum:false}),
        super::v2_wallet_sim::Wire::Vista(v) if matches!(v.form,super::super::exec::vista_wire::Form::SurgeModuleEthIn|super::super::exec::vista_wire::Form::SurgeModuleEthOut)=>Some(RouterFeeMode::NativeWallet{target:execution.target,native_cash:super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt)}),
        super::v2_wallet_sim::Wire::Vista(v) if v.form==super::super::exec::vista_wire::Form::SurgeToken=>Some(RouterFeeMode::ProtectedWallet{target:execution.target}),
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
    let partial_input=c.entry.family.variant.llamma() || c.entry.family.variant==super::super::pools::family_variant::Variant::Ambient;
    if c.coins != i.qty && !(partial_input && i.side==Side::Sell && c.coins>Decimal::ZERO && c.coins<i.qty) {
        return Err(bad());
    }
    if let crate::core::types::PoolState::BancorV3(state)=&c.entry.pool.state && i.side==Side::Buy {
        let want=direct::ceil_units(i.qty,i.coin_dec).ok_or_else(bad)?;
        let fill=match &execution.wire{
            v2_wallet_sim::Wire::BancorV3(t) if t.desired.is_some()=>state.swap(i.usdt,false,want,t.exemption),
            _=>state.input_for_forward(i.usdt,want,false),
        }.ok_or_else(bad)?;
        c.swapped.amount_in=fill.spent;
        c.swapped.amount_out=fill.received;
        c.usdt=num::to_decimal(fill.spent,i.usdt_dec).ok_or_else(bad)?;
    }
    if let v2_wallet_sim::Wire::Stabull(t)=&execution.wire && let Some(want)=t.desired{
        let head=h.ok_or_else(bad)?;
        if i.side!=Side::Buy{return Err(bad())}
        let debit=super::stabull_pricing::debit(ctx,head,*t,payer,i.usdt,i.coin,want).await?;
        c.swapped.amount_in=debit;c.swapped.amount_out=want;c.usdt=num::to_decimal(debit,i.usdt_dec).ok_or_else(bad)?;
    }
    if let super::v2_wallet_sim::Wire::Integral(t)=&execution.wire && let (Some(want),Some(head))=(t.desired,h){
        let need=super::super::pools::integral_quotes::required(ctx,t.relay,i.usdt,i.coin,want,&head.selector()).await?;
        if i.side!=Side::Buy{return Err(bad())}
        c.swapped.amount_in=need;c.swapped.amount_out=want;c.usdt=num::to_decimal(need,i.usdt_dec).ok_or_else(bad)?;
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
            super::super::exec::native::is_native(ctx, &i.asset) || super::vista_pricing::native_cash_target(route).is_some(),
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
        let vista_value = if let super::v2_wallet_sim::Wire::BancorLegacy(t)=&execution.wire{Some(t.value(input,amount))}else if let super::v2_wallet_sim::Wire::Mooniswap(t)=&execution.wire{Some(t.value(input,amount))}else if let super::v2_wallet_sim::Wire::DefiPlaza(t)=&execution.wire{Some(t.value(input,amount))}else if let super::v2_wallet_sim::Wire::YieldBasis(t)=&execution.wire{Some(t.value(input,amount))}else if let super::v2_wallet_sim::Wire::BalancerV1(t)=&execution.wire{Some(t.value(input,amount))}else if let super::v2_wallet_sim::Wire::BancorV3(t)=&execution.wire{Some(t.value(amount))}else if let super::v2_wallet_sim::Wire::UniswapV1(t)=&execution.wire{Some(t.value(amount))}else if let super::v2_wallet_sim::Wire::NativeFlat(t)=&execution.wire{Some(t.wei)}else if let super::v2_wallet_sim::Wire::FluxEth(m)=&execution.wire{Some(if input==m.wrapped{amount}else{U256::ZERO})} else if let super::v2_wallet_sim::Wire::Carbon(c)=&execution.wire{Some(c.value())}else if let super::v2_wallet_sim::Wire::EkuboV2(key,Some(_))=&execution.wire {Some(if key.side{U256::ZERO}else{amount})}else if super::vista_pricing::is_vista(&c.entry) || matches!(&execution.wire,super::v2_wallet_sim::Wire::Vista(_)) {
            Some(match &execution.wire {
                super::v2_wallet_sim::Wire::Vista(call) => call.value(amount),
                super::v2_wallet_sim::Wire::V5(v) if matches!(v.native, Some((_, true))) => amount,
                _ => U256::ZERO,
            })
        } else {
            None
        };
        let algebra_wallet=matches!(&execution.wire,v2_wallet_sim::Wire::RegisteredV3(m) if m.algebra_wallet.is_some());
        let o = Order {
            payer,
            target,
            input,
            output,
            amount,
            minimum,
            wire: execution.wire,
        };
        let fill = if let Some(fill)=measured.filter(|_|i.side==Side::Sell){fill}else{v2_wallet_sim::measure(
            ctx,
            head,
            o,
            super::oasis_pricing::is_oasis(&c.entry) || super::vista_pricing::is_vista(&c.entry)
                || super::vault_pricing::enabled(&c.entry) || super::module_pricing::enabled(&c.entry) || vista_value.is_some() || c.entry.family.variant.exact_router() || c.entry.family.variant==super::super::pools::family_variant::Variant::SafeSwapV2 || super::ekubo_v2_pricing::enabled(&c.entry) || super::super::exec::mint_club_direct::enabled(&c.entry),
        )
        .await?};
        if let Some(value) = vista_value {
            Box::pin(super::vista_cash::funded(
                q, head, payer, value, &fill, i.gas,
            ))
            .await?;
        }
        if i.side == Side::Sell {
            c.usdt = num::to_decimal(fill.received, i.usdt_dec).ok_or_else(bad)?;
            if algebra_wallet || partial_input {c.coins=num::to_decimal(fill.charged,i.coin_dec).ok_or_else(bad)?.min(i.qty);}
        } else if algebra_wallet || partial_input {
            c.usdt=num::to_decimal(fill.charged,i.usdt_dec).ok_or_else(bad)?;
        }
        c.usable.gas = fill.swap_gas;
        approval = Some(fill.approval);
    }
    let mut quote = build(c)?;
    if let Some(RouterFeeMode::NativeFlat{wei,..})=fee_mode && !wei.is_zero(){
        quote.costs.push(crate::core::types::Cost::new(crate::core::types::CostKind::ProtocolFee,num::to_decimal(wei,18).ok_or_else(bad)?,gas_t.clone(),false));
    }
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
