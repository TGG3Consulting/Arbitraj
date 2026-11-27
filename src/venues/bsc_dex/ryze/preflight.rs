//! Перед подписью повторно проверяет весь маршрут на одном каноническом блоке под блокировкой кошелька.
use super::super::{
    ctx::Ctx,
    node::{abi::IERC20, call::canonical},
    quote::white_state,
};
use super::{
    market::Ticket,
    tokens,
    wire::{self, SwapIntent},
};
use crate::gateway::errors::GatewayResult;

pub(super) async fn eligible(
    ctx: &Ctx,
    ticket: &Ticket,
    order: &SwapIntent,
) -> GatewayResult<bool> {
    let h = white_state::head(ctx, "latest").await?;
    let at = h.selector();
    let c = &ticket.config;
    let who = order.user;
    let ((pools, fee), nonce, balance, allow, valid) = tokio::try_join!(
        c.verify(ctx, h),
        canonical(ctx, &at, c.router, wire::swapNoncesCall { user: who }),
        canonical(
            ctx,
            &at,
            order.tokenIn,
            IERC20::balanceOfCall { owner: who }
        ),
        canonical(
            ctx,
            &at,
            order.tokenIn,
            IERC20::allowanceCall {
                owner: who,
                spender: c.router
            }
        ),
        canonical(
            ctx,
            &at,
            c.router,
            wire::validatePathCall {
                path: order.path.clone()
            }
        )
    )?;
    Ok(valid
        && fee == ticket.usd_fee
        && nonce == order.nonce
        && balance.min(allow) >= order.amountIn
        && order.matches(who, &pools, h.timestamp.to())
        && tokens::physical(ctx, c, &at, &order.path, who).await)
}
