//! Одиночная нога по стандартному V3. Сравнение чистого результата штатного Router и шага V5.
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::{V5Step, Wire},
    white_state::Head,
};
use crate::{
    core::types::{RouteMode, Side},
    venues::bsc_dex::{
        exec::{executor, registered_v3_wallet::Market},
        pools::{family_variant::Variant, registered_router},
        wallet::signer,
    },
};
use alloy_primitives::Bytes;

pub(super) async fn candidates(q: &Quoting, i: &Input, head: Option<Head>) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let Ok(payer) = signer::address(ctx) else {
        return Vec::new();
    };
    let executor = if ctx.text("dex.router_mode").is_ok_and(|s| s == "graph")
        && !crate::gateway::quote_stage::settlement()
    {
        executor::address(ctx).ok().flatten()
    } else {
        None
    };
    let (input, output) = if i.side == Side::Buy {
        (i.usdt, i.coin)
    } else {
        (i.coin, i.usdt)
    };
    let entries = i.entries.iter().filter(|e| e.family.variant.exact_router());
    let mut options = Vec::new();
    for e in entries {
        let pool = e.pool.reference.address();
        let classic = matches!(e.family.variant,Variant::SolidlyProxy|Variant::SolidlyPairOverride);
        let router = if (e.family.variant==Variant::SolidlyPairOverride && super::super::pools::exact_wallet_tokens::eligible_input(ctx,e.family.id,input)) || registered_router::eligible(ctx, e.family.id, input, output) {
            match head {
                Some(h) if classic => Box::pin(super::super::pools::solidly_proxy::router(
                    ctx,
                    e,
                    input,
                    output,
                    &h.selector(),
                ))
                .await
                .ok(),
                Some(h) => registered_router::checked(ctx, e, input, output, &h.selector())
                    .await
                    .ok(),
                None => registered_router::target(ctx,e).ok(),
            }
        } else {
            None
        };
        let plain=e.family.variant==Variant::V2ExactRouter;
        let wire = if plain {
            Some(Wire::SolidlyRouter(super::super::exec::solidly_router_wallet::Market{pool,stable:None,supporting_fee:false}))
        } else if classic {
            match &e.pool.state {
                crate::core::types::PoolState::Solidly { stable, .. } => Some(Wire::SolidlyRouter(
                    super::super::exec::solidly_router_wallet::Market {
                        pool,
                        stable: Some(*stable),
                        supporting_fee:e.family.variant==Variant::SolidlyPairOverride,
                    },
                )),
                _ => None,
            }
        } else {
            Market::from_entry(e).map(Wire::RegisteredV3)
        };
        if let (Some(target), Some(wire)) = (router, wire) {
            let order = Execution {
                mode: RouteMode::Direct,
                target,
                wire,
            };
            if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, order, head).await {
                options.push(option);
            }
        }
        if let Some(target)=registered_router::wallet_module(ctx,e) && let Some(mut m)=Market::from_entry(e)
            && (head.is_none() || registered_router::checked_module(ctx,e,&head.unwrap().selector()).await.is_ok()){
            m.wallet_module=true;
            let order=Execution{mode:RouteMode::Direct,target,wire:Wire::RegisteredV3(m)};
            if let Ok(option)=v2_execution::option(q,i,e.clone(),payer,order,head).await{options.push(option)}
        }
        if let Some(target) = executor && e.family.variant != Variant::VvsV3 {
            let (kind,data)=if plain {
                use alloy::sol_types::SolValue;
                let Some(fee)=e.pool.fee_pips else {continue};
                (0,Bytes::from(fee.abi_encode()))
            } else {(if classic {3} else {1},Bytes::new())};
            let wire = Wire::V5(V5Step {
                kind,
                pool,
                data,
                native: None,
            });
            let order = Execution {
                mode: RouteMode::Graph,
                target,
                wire,
            };
            if let Ok(option) = v2_execution::option(q, i, e.clone(), payer, order, head).await {
                options.push(option);
            }
        }
    }
    options
}
