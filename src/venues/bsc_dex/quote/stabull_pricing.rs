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
        call::canonical(
            ctx,
            &at,
            terms.pool,
            IStabull::viewTargetSwapCall {
                a: input,
                b: output,
                amount: want
            }
        ),
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
    if exact_wallet_tokens::eligible(ctx, e.family.id, input, output) {
        targets.push(pool);
        if let Ok(router) = ctx.dex_addr("dex.quoter", e.family.id) {
            targets.push(router);
        }
    }
    if e.family.executor_kind.is_some()
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
        if t.form == Form::Module
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
