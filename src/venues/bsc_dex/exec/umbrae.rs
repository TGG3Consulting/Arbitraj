//! Собирает подписываемый одиночный обмен для выбранного в котировке получателя allowance.
use super::{
    chain::Step,
    plan::{DirectCall, Plan},
    white_direct,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, pools::umbrae, quote::white_state},
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
sol! {interface IWallet{function swapExactTokensForTokens(uint256 amount,uint256 minimum,address[] path,uint16[] steps,address[] pools,address recipient,uint256 deadline) external returns(uint256);}}

#[derive(Clone, Copy, Debug)]
pub(in crate::venues::bsc_dex) struct Path {
    pub pool: Address,
    pub step: u16,
}
impl Path {
    pub(in crate::venues::bsc_dex) fn data(
        self,
        input: Address,
        output: Address,
        amount: U256,
        minimum: U256,
        recipient: Address,
        deadline: U256,
    ) -> Bytes {
        IWallet::swapExactTokensForTokensCall {
            amount,
            minimum,
            path: vec![input, output],
            steps: vec![self.step],
            pools: vec![self.pool],
            recipient,
            deadline,
        }
        .abi_encode()
        .into()
    }
}

pub(in crate::venues::bsc_dex) async fn plan(
    ctx: &Ctx,
    s: &Step,
    wallet: Address,
    height: u64,
    selected: Option<Address>,
    all_approvals: bool,
) -> GatewayResult<Plan> {
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Umbrae: не подтверждён выбранный адрес исполнения ERC-20",
        )
    };
    let target = selected.ok_or_else(fail)?;
    let generation = ctx.settings.generation();
    let head =
        white_state::head(ctx, &crate::venues::bsc_dex::node::call::block_tag(height)).await?;
    let at = head.selector();
    let e = &s.entry;
    let pool = e.pool.reference.address();
    let actual = umbrae::market(ctx, e.family, pool, (e.coin, e.usdt), &at).await?;
    let direct = if Some(target) == actual.router {
        DirectCall::Umbrae {
            path: Path {
                pool,
                step: actual.step.ok_or_else(fail)?,
            },
            input: s.order.token_in,
            amount: s.order.amount_in,
        }
    } else {
        if !e.family.tradable || ctx.dex_addr("dex.router", e.family.id)? != target {
            return Err(fail());
        }
        umbrae::module(ctx, e.family, target, &at).await?;
        DirectCall::Module {
            pool,
            input: s.order.token_in,
            amount: s.order.amount_in,
        }
    };
    let mut p = white_direct::build(
        ctx,
        s,
        wallet,
        target,
        head.timestamp.to::<u64>(),
        all_approvals,
        generation,
    )
    .await?;
    p.set_direct_market(direct);
    Ok(p)
}
