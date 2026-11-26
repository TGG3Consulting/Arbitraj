//! Однопуловый обмен EOA через WhiteSwap с проверкой поступления на кошелёк.
//! Штатные стадии разрешений, повторной симуляции, подписи и сверки сохраняются.
use super::super::{ctx::Ctx, quote::white_state, wallet::approve};
use super::{
    chain::Step,
    plan::{DirectCall, Plan},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy_primitives::{Address, U256};

tokio::task_local! {pub(super) static SCOPE:();}
pub(in crate::venues::bsc_dex) fn selected() -> bool {
    SCOPE.try_with(|_| ()).is_ok()
}

pub(super) async fn plan(
    ctx: &Ctx,
    steps: &[Step],
    wallet: Address,
    now: u64,
    all: bool,
) -> GatewayResult<Plan> {
    let [s] = steps else {
        return Err(ctx.err(ErrorCode::NoRoute, "WhiteSwap EOA требует один пул"));
    };
    if !s.to_wallet || !s.from_wallet {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "WhiteSwap: требуется ERC-20 кошелька для обеих сторон",
        ));
    }
    let generation = ctx.settings.generation();
    let h = white_state::head(ctx, "latest").await?;
    let router = ctx.dex_addr("dex.router", s.entry.family.id)?;
    if super::super::quote::router_pricing::is_native(&s.entry) {
        super::super::quote::router_pricing::checked(
            ctx,
            &s.entry,
            s.order.token_in,
            s.order.amount_in,
            h,
        )
        .await?;
    } else {
        let pips = white_state::verified(ctx, &s.entry, wallet, h).await?;
        if s.entry.pool.fee_pips.is_some_and(|quoted| quoted < pips) {
            return Err(ctx.err(
                ErrorCode::StaleData,
                "WhiteSwap: скидка плательщика уменьшилась",
            ));
        }
    }
    build(ctx, s, wallet, router, now, all, generation).await
}

/// Собирает единственный обмен Router02 только после проверки роутера вызывающим адаптером.
pub(super) async fn build(
    ctx: &Ctx,
    s: &Step,
    wallet: Address,
    router: Address,
    now: u64,
    all: bool,
    generation: u64,
) -> GatewayResult<Plan> {
    if !s.from_wallet || !s.to_wallet {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Router02: обе стороны должны принадлежать кошельку",
        ));
    }
    let o = &s.order;
    let approvals = if all {
        approve::fork_all(ctx, o.token_in, router, o.amount_in)?
    } else {
        approve::fork_missing(ctx, o.token_in, wallet, router, o.amount_in).await?
    };
    if ctx.settings.generation() != generation {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Router02: изменились настройки при подготовке",
        ));
    }
    let end = U256::from(now + ctx.int("dex.deadline_s")?);
    let mut plan = Plan::new(
        ctx,
        (approvals, router, wallet),
        o.token_out,
        vec![],
        o.min_out,
        end,
    )?;
    plan.direct_market = Some(DirectCall::V2SupportingFee {
        input: o.token_in,
        amount: o.amount_in,
    });
    Ok(plan)
}
