//! Сравнивает прямой вызов проверенного модуля рынка и шаг V5 по чистой выплате и расходам текущего кошелька.
use super::super::{
    exec::executor,
    pools::{family_variant::Variant, memebox, registered_v3, registry::Entry},
    wallet::signer,
};
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::{V5Step, Wire},
    white_state::Head,
};
use crate::{core::types::RouteMode, gateway::quote_stage};
use alloy_primitives::Bytes;

pub(super) fn enabled(e: &Entry) -> bool {
    (matches!(e.family.variant,Variant::Pendle | Variant::SafeSwapTrade | Variant::DegenSwap) && e.family.executor_kind.is_some()) || matches!(e.family.variant,Variant::Angstrom|Variant::BancorLegacy|Variant::YieldBasis|Variant::BalancerV1|Variant::BancorV3|Variant::UniswapV1|Variant::RubiconClassic|Variant::PropVenue|Variant::Stabull|Variant::IntegralSize|Variant::Dagrow|Variant::Bundles) || super::super::exec::o1_wallet::enabled(e) || super::super::exec::antfarm_wallet::enabled(e) || super::super::exec::temple_wallet::enabled(e) || !e.family.variant.umbrae() && e.family.variant.wallet_module() && e.family.tradable && e.family.executor_kind.is_some()
}

pub(super) fn native_module(v:Variant)->bool {
    v.flux_native() || matches!(v,Variant::UniswapV1|Variant::BancorV3|Variant::BalancerV1|Variant::BancorLegacy|Variant::YieldBasis)
}

pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let Ok(payer) = signer::address(ctx) else {
        return Vec::new();
    };
    let graph =
        !quote_stage::settlement() && ctx.text("dex.router_mode").ok().as_deref() == Some("graph");
    let mut quotes = Vec::new();
    let native=super::super::exec::native::is_native(ctx,&i.asset) || super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt);
    for e in i.entries.iter().filter(|e| enabled(e) && (!native || native_module(e.family.variant))) {
        if e.family.variant==Variant::BancorLegacy {
            quotes.extend(Box::pin(super::bancor_legacy_pricing::options(q,i,e,payer,native,graph,head)).await);
            continue;
        }
        if e.family.variant==Variant::YieldBasis {
            quotes.extend(Box::pin(super::yield_basis_pricing::options(q,i,e,payer,native,graph,head)).await);
            continue;
        }
        if e.family.variant==Variant::BalancerV1 {
            quotes.extend(Box::pin(super::balancer_v1_pricing::options(q,i,e,payer,native,graph,head)).await);
            continue;
        }
        if matches!(e.pool.state,crate::core::types::PoolState::BancorV3(_)){
            quotes.extend(Box::pin(super::bancor_v3_pricing::options(q,i,e,payer,native,graph,head)).await);
            continue;
        }
        if e.family.variant==Variant::Angstrom {
            if let Some(h)=head && let Ok(target)=ctx.dex_addr("dex.router",e.family.id)
                && let Ok(t)=Box::pin(super::super::exec::angstrom_plan::checked(ctx,e,h,target)).await
                && let Ok(row)=Box::pin(v2_execution::option(q,i,e.clone(),payer,Execution{mode:RouteMode::Direct,target,wire:Wire::Angstrom(std::sync::Arc::new(t))},Some(h))).await {quotes.push(row);}
            continue;
        }
        if e.family.variant==Variant::Stabull {
            quotes.extend(Box::pin(super::stabull_pricing::options(q,i,e,payer,graph,head)).await);
            continue;
        }
        if matches!(e.pool.state,crate::core::types::PoolState::UniswapV1(_)) {
            quotes.extend(Box::pin(super::uniswap_v1_pricing::options(q,i,e,payer,native,graph,head)).await);
            continue;
        }
        if matches!(e.family.variant,Variant::Pendle | Variant::SafeSwapTrade | Variant::DegenSwap){
            let safe=e.family.variant.protected_router_wallet();
            if e.family.variant==Variant::DegenSwap {
                let src=if i.side==crate::core::types::Side::Buy{i.usdt}else{i.coin};
                if super::super::pools::exact_wallet_tokens::eligible_input(ctx,e.family.id,src) {
                    let target=match head{
                        Some(h)=>super::super::exec::degen_router::checked(ctx,e,src,&h.selector()).await.ok(),
                        None=>ctx.dex_addr("dex.quoter",e.family.id).ok(),
                    };
                    if let Some(target)=target{
                        let native=Execution{mode:RouteMode::Direct,target,wire:Wire::DegenRouter(e.pool.reference.address())};
                        if let Ok(v)=v2_execution::option(q,i,e.clone(),payer,native,head).await{quotes.push(v)}
                    }
                }
            }
            let Ok(target)=ctx.dex_addr("dex.router",e.family.id) else{continue};
            if super::super::exec::native::is_native(ctx,&i.asset) || super::vista_pricing::cash(ctx,&i.usdt_ticker,i.usdt){continue}
            let input=if i.side==crate::core::types::Side::Buy{i.usdt}else{i.coin};
            if let Some(h)=head {
                let ok=if safe {Box::pin(super::super::pools::safeswap_trade::market(ctx,e.family,e.pool.reference.address(),(e.pool.token0,e.pool.token1),&h.selector())).await.is_ok()}
                    else {Box::pin(super::super::pools::pendle_module::checked(ctx,e,input,h)).await.is_ok()};
                if !ok {continue}
            }
            let market=e.pool.reference.address();
            let direct=Execution{mode:RouteMode::Direct,target,wire:if e.family.variant==Variant::DegenSwap{Wire::DegenModule(market)}else if safe{Wire::SafeSwap(market)}else{Wire::Pendle(market)}};
            if let Ok(v)=v2_execution::option(q,i,e.clone(),payer,direct,head).await{quotes.push(v)}
            if graph && let Ok(Some(v5))=executor::address(ctx){
                let step=if safe{V5Step{native:None,kind:29,pool:target,data:Bytes::new()}}else{V5Step{native:None,kind:17,pool:target,data:Bytes::copy_from_slice(market.into_word().as_slice())}};
                let via=Execution{mode:RouteMode::Graph,target:v5,wire:Wire::V5(step)};
                if let Ok(v)=v2_execution::option(q,i,e.clone(),payer,via,head).await{quotes.push(v)}
            }continue
        }
        if let crate::core::types::PoolState::Rubicon(book)=&e.pool.state {
            let buy=i.side==crate::core::types::Side::Buy;
            let input=if buy{i.usdt}else{i.coin};
            if let Some(amount)=super::super::num::to_units(i.qty,i.coin_dec)
                && let Ok(t)=super::super::exec::rubicon_plan::terms(&q.ctx,e,book,input,amount,buy.then_some(amount))
                && (match head {Some(h)=>super::super::exec::rubicon_plan::check(&q.ctx,e,&t,&h.selector()).await.is_ok(),None=>true})
            {
                let exec=Execution{mode:RouteMode::Direct,target:t.target,wire:Wire::Rubicon(Box::new(t))};
                if let Ok(v)=Box::pin(v2_execution::option(q,i,e.clone(),payer,exec,head)).await{quotes.push(v)}
            }
            continue;
        }
        if e.family.variant==Variant::PropVenue{quotes.extend(Box::pin(super::prop_pricing::options(q,i,e,payer,head)).await);continue}
        if e.family.variant==Variant::IntegralSize{quotes.extend(Box::pin(super::integral_pricing::options(q,i,e,payer,graph,head)).await);continue}
        if e.family.variant==Variant::Bundles{
            quotes.extend(Box::pin(super::bundles_pricing::options(q,i,e,payer,graph,head)).await);
            continue;
        }
        if e.family.variant==Variant::Dagrow{
            quotes.extend(Box::pin(super::dagrow_pricing::options(q,i,e,payer,graph,head)).await);
            continue;
        }
        if super::super::exec::antfarm_wallet::enabled(e){
            quotes.extend(Box::pin(super::antfarm_pricing::options(q,i,e,head)).await);
            continue;
        }
        if super::super::exec::o1_wallet::enabled(e) && native {
            if e.family.variant.flux_native() {
                for module in [false,true] {
                    let Ok(m)=super::super::exec::flux_eth::Market::read(ctx,e,module) else {continue};
                    if !module && !super::super::pools::exact_wallet_tokens::eligible_input(ctx,e.family.id,m.token) {continue;}
                    if module && ctx.dex_text("dex.infinity_native_module_hash",e.family.id).ok().and_then(|h|h.parse::<alloy_primitives::B256>().ok()).is_none(){continue;}
                    if let Some(h)=head && super::super::exec::flux_eth_plan::checked(ctx,e,&h.selector(),module).await.is_err(){continue;}
                    let execution=Execution{mode:RouteMode::Direct,target:m.router,wire:Wire::FluxEth(m)};
                    if let Ok(option)=v2_execution::option(q,i,e.clone(),payer,execution,head).await {quotes.push(option);}
                }
            }
            continue;
        }
        let Ok(module) = ctx.dex_addr("dex.router", e.family.id) else {
            continue;
        };
        let mut e = e.clone();
        if let Some(h) = head {
            let check = if super::super::exec::o1_wallet::enabled(&e) {
                super::super::exec::o1_wallet::at(ctx,&e,&h.selector()).await
            } else if e.family.variant==Variant::Temple {
                super::super::pools::temple::market(ctx,e.family,e.pool.reference.address(),(e.coin,e.usdt),&h.selector()).await.map(|s|e.pool.state=crate::core::types::PoolState::Temple(Box::new(s)))
            } else if e.family.variant==Variant::MaverickOne {
                super::super::pools::maverick_one::market(ctx,e.family,e.pool.reference.address(),(e.pool.token0,e.pool.token1),&h.selector()).await.map(|_|())
            } else if e.family.variant == Variant::BunnySwap {
                super::super::pools::bunnyswap::fresh(ctx,&e,&h.selector()).await
            } else if e.family.variant == Variant::CandySwap {
                super::super::pools::v2_live_fee::candy_fresh(ctx,&e,&h.selector()).await
            } else if e.family.variant.registered_module() {
                registered_v3::fresh(ctx, &e, &h.selector()).await
            } else {
                memebox::refresh(ctx, &mut e, h).await
            };
            if check.is_err() {
                continue;
            }
        }
        let o1 = if super::super::exec::o1_wallet::enabled(&e) {
            let Ok(id) = super::super::exec::o1_wallet::key(ctx,&e) else {continue};
            Some(id)
        } else {None};
        let direct = Execution {
            mode: RouteMode::Direct,
            target: module,
            wire: if let Some(id)=o1 {Wire::O1(id)} else if let crate::core::types::PoolState::Temple(s)=&e.pool.state{
                Wire::Temple(std::sync::Arc::new(super::super::exec::temple_wallet::Key::new(s,module)))
            }else{Wire::DirectModule{pool:e.pool.reference.address()}},
        };
        if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, direct, head).await {
            quotes.push(option);
        }
        if graph && let Ok(Some(target)) = executor::address(ctx) {
            let (kind, data) = if let Some(id)=o1 {(17,Bytes::copy_from_slice(id.as_slice()))} else if e.family.variant.pool_address_step() {
                (
                    17,
                    Bytes::copy_from_slice(e.pool.reference.address().into_word().as_slice()),
                )
            } else {
                (29, Bytes::new())
            };
            let step = V5Step {
                native: None,
                kind,
                pool: module,
                data,
            };
            let via = Execution {
                mode: RouteMode::Graph,
                target,
                wire: if e.family.variant.pool_address_step(){
                    Wire::funded(step,vec![e.pool.reference.address()],None)
                }else{Wire::V5(step)},
            };
            if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, via, head).await {
                quotes.push(option);
            }
        }
    }
    quotes
}
