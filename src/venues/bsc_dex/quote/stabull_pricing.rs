//! Сравнение защищённой Curve с точным входом/выходом и двух штатных входов для проверенных ERC20.
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    v2_wallet_sim::{self, V5Step, Wire},
    white_state::Head,
};
use crate::{
    core::types::{RouteMode, Side},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        exec::{
            executor, stabull_plan,
            stabull_wallet::{Form, Terms},
        },
        node::call,
        pools::{dagrow, exact_wallet_tokens, registry::Entry, stabull::IStabull},
    },
};
use alloy_primitives::{Address, Bytes, U256};

pub(super) async fn debit(
    ctx: &Ctx,
    head: Head,
    terms: Terms,
    payer: Address,
    input: Address,
    output: Address,
    want: U256,
) -> GatewayResult<U256> {
    let at = head.selector();
    let (nominal, balance) = tokio::try_join!(
        async{
            if matches!(terms.form,Form::Euler{..}){
                call::canonical(ctx,&at,terms.pool,crate::venues::bsc_dex::pools::euler_swap::IEuler::computeQuoteCall{
                    input,output,amount:want,exactIn:false}).await
            }else{call::canonical(ctx,&at,terms.pool,IStabull::viewTargetSwapCall{a:input,b:output,amount:want}).await}
        },
        dagrow::balance(ctx, input, payer, &at)
    )?;
    if nominal.is_zero() || balance.is_zero() {
        return Err(stabull_plan::bad(ctx));
    }
    let measure = |cap| {
        v2_wallet_sim::measure(
            ctx,
            head,
            v2_wallet_sim::Order {
                payer,
                target: terms.target,
                input,
                output,
                amount: cap,
                minimum: want,
                wire: Wire::Stabull(terms),
            },
            false,
        )
    };
    match measure(nominal.min(balance)).await {
        Ok(fill) => Ok(fill.charged),
        Err(e)
            if balance > nominal
                && matches!(e.code, ErrorCode::QuoteMismatch | ErrorCode::SwapReverted) =>
        {
            // Лишь проба на каноническом состоянии. Подписывается бюджет, повторно проверенный по фактическому списанию.
            Ok(measure(balance).await?.charged)
        }
        Err(e) => Err(e),
    }
}
pub(super) async fn options(
    q: &Quoting,
    i: &Input,
    e: &Entry,
    payer: Address,
    graph: bool,
    h: Option<Head>,
) -> Vec<OptionQuote> {
    let ctx = &q.ctx;
    let Some(h) = h else { return Vec::new() };
    let at = h.selector();
    let buy = i.side == Side::Buy;
    let (input, output) = if buy {
        (i.usdt, i.coin)
    } else {
        (i.coin, i.usdt)
    };
    let want = buy
        .then(|| super::direct::ceil_units(i.qty, i.coin_dec))
        .flatten();
    let pool = e.pool.reference.address();
    let mut rows = Vec::new();
    let mut targets = Vec::new();
    if graph && e.family.variant.euler() && let Ok(Some(executor))=executor::address(ctx)
        && let Some((key,manager))=crate::venues::bsc_dex::pools::euler_hook::step(ctx,e)
        && crate::venues::bsc_dex::pools::euler_hook::canonical(ctx,e.family,pool,(e.pool.token0,e.pool.token1),&at).await.ok()==e.key {
        let step=V5Step{native:None,kind:19,pool:manager,data:key.pool_key().abi().into()};
        let route=Execution{mode:RouteMode::Graph,target:executor,wire:Wire::funded(step,vec![pool],None)};
        if let Ok(option)=v2_execution::option(q,i,e.clone(),payer,route,Some(h)).await {rows.push(option);}
    }
    if exact_wallet_tokens::eligible(ctx, e.family.id, input, output) {
        if !e.family.variant.euler(){targets.push(pool);}
        if let Ok(router) = ctx.dex_addr("dex.quoter", e.family.id) {
            targets.push(router);
        }
    }
    let module=e.family.executor_kind.is_some_and(|k| !e.family.variant.euler() || k==crate::venues::bsc_dex::pools::exec_kind::ExecKind::Balancer);
    if module
        && let Ok(module) = ctx.dex_addr("dex.router", e.family.id)
    {
        targets.push(module);
    }
    for target in targets {
        let Ok(t) = stabull_plan::checked(ctx, e, (input, output), target, None, &at).await else {
            continue;
        };
        for desired in
            std::iter::once(None).chain(want.filter(|_| t.form != Form::Router).map(Some))
        {
            let wire = Wire::Stabull(Terms { desired, ..t });
            let route = Execution {
                mode: RouteMode::Direct,
                target,
                wire,
            };
            if let Ok(qt) = v2_execution::option(q, i, e.clone(), payer, route, Some(h)).await {
                rows.push(qt);
            }
        }
        if matches!(t.form,Form::Module | Form::Euler{guarded:true,..})
            && graph
            && let Ok(Some(executor)) = executor::address(ctx)
        {
            let step = V5Step {
                native: None,
                kind: 17,
                pool: target,
                data: Bytes::copy_from_slice(pool.into_word().as_slice()),
            };
            let route = Execution {
                mode: RouteMode::Graph,
                target: executor,
                wire: Wire::funded(step, vec![pool], None),
            };
            if let Ok(qt) = v2_execution::option(q, i, e.clone(), payer, route, Some(h)).await {
                rows.push(qt);
            }
        }
    }
    rows
}
