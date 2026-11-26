//! Читает площадку LunarBase в закреплённом состоянии, отдельно строит котировки покупки и продажи.
use super::{ILunar, Profile, Proto, verify_at, view};
use crate::core::types::{LadderPoint, OraclePmmData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    math::full_math::mul_div,
    pools::{direct_module, elfomo, families::Family, ladder_counter, registry::Registry},
    tokens::Tokens,
    wallet::signer,
};
use alloy_primitives::{Address, U256};
use serde_json::Value;

struct Reader<'a> {
    ctx: &'a Ctx,
    tag: &'a Value,
    owner: Address,
    target: Address,
    profile: Profile,
}

impl<'a> Reader<'a> {
    async fn new(ctx: &'a Ctx, id: &str, profile: Profile, tag: &'a Value) -> GatewayResult<Self> {
        verify_at(ctx, &profile, tag).await?;
        let target = if ctx.dex_text("dex.kind", id)? == "lunarbase_module" {
            let module = ctx.dex_addr("dex.router", id)?;
            direct_module::verify_code(ctx, id, module, tag).await?;
            module
        } else {
            profile.pool
        };
        Ok(Self {
            ctx,
            tag,
            owner: signer::address(ctx)?,
            target,
            profile,
        })
    }
    async fn ask(&self, from: Address, to: Address, size: U256) -> GatewayResult<U256> {
        if self.target == self.profile.pool {
            let token = |a| {
                if a == self.profile.bases[0] {
                    Address::ZERO
                } else {
                    a
                }
            };
            return view(
                self.ctx,
                self.tag,
                self.owner,
                self.profile.pool,
                ILunar::quoteExactInCall {
                    tokenIn: token(from),
                    tokenOut: token(to),
                    amountIn: size,
                },
            )
            .await;
        }
        view(
            self.ctx,
            self.tag,
            self.owner,
            self.target,
            ILunar::quoteLunarCall {
                tokenIn: from,
                tokenOut: to,
                amountIn: size,
            },
        )
        .await
    }
    async fn side(
        &self,
        buy: bool,
        sizes: &[U256],
        anchor: U256,
        reserve_out: U256,
    ) -> GatewayResult<Vec<LadderPoint>> {
        let mut points: Vec<LadderPoint> = Vec::new();
        let q96 = U256::ONE << 96u32;
        for &size in sizes {
            let (a, b, dx) = if buy {
                (self.profile.counter, self.profile.bases[0], size)
            } else {
                let Some(dx) = mul_div(size, q96, anchor).and_then(|v| mul_div(v, q96, anchor))
                else {
                    break;
                };
                (self.profile.bases[0], self.profile.counter, dx)
            };
            if dx.is_zero() {
                break;
            }
            let out = match self.ask(a, b, dx).await {
                Ok(out) => out,
                Err(e) if e.code == ErrorCode::SwapReverted => break,
                Err(e) => return Err(e),
            };
            if out.is_zero()
                || out > reserve_out
                || points
                    .last()
                    .is_some_and(|p| dx <= p.amount_in || out <= p.amount_out)
            {
                break;
            }
            points.push(LadderPoint {
                amount_in: dx,
                amount_out: out,
            });
        }
        Ok(points)
    }
}

pub async fn quote(
    ctx: &Ctx,
    id: &str,
    from: Address,
    to: Address,
    dx: U256,
    at: &str,
) -> GatewayResult<U256> {
    let profile = Profile::read(ctx, id, Proto::Lunar)?;
    if !profile.unordered(from, to) {
        return Err(ctx.err(ErrorCode::NoRoute, "Пара LunarBase не подтверждена"));
    }
    let tag = elfomo::block(ctx, at).await?;
    let r = Reader::new(ctx, id, profile, &tag).await?;
    if super::open(ctx, &tag, r.owner, r.profile.pool).await? {
        let out = r.ask(from, to, dx).await?;
        if !out.is_zero() {
            r.profile.current(ctx)?;
            return Ok(out);
        }
    }
    Err(ctx.err(
        ErrorCode::NoRoute,
        "LunarBase приостановлен, цена устарела или объём неисполним",
    ))
}

pub async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    reference: Address,
    ordered: (Address, Address),
    at: &str,
) -> GatewayResult<PoolState> {
    let profile = Profile::read(ctx, f.id, Proto::Lunar)?;
    if reference != profile.pool || !profile.unordered(ordered.0, ordered.1) {
        return Err(ctx.err(ErrorCode::NotApplicable, "Неизвестный рынок LunarBase"));
    }
    let tag = elfomo::block(ctx, at).await?;
    let r = Reader::new(ctx, f.id, profile, &tag).await?;
    let (active, state, rx, ry) = tokio::try_join!(
        super::open(ctx, &tag, r.owner, r.profile.pool),
        view(ctx, &tag, r.owner, r.profile.pool, ILunar::stateCall {}),
        view(
            ctx,
            &tag,
            r.owner,
            r.profile.pool,
            ILunar::getXReserveCall {}
        ),
        view(
            ctx,
            &tag,
            r.owner,
            r.profile.pool,
            ILunar::getYReserveCall {}
        )
    )?;
    let anchor = U256::from(state.anchorPrice);
    let (buy, sell) = if active && !anchor.is_zero() {
        let sizes = ladder_counter::amounts(ctx, tokens, reg, f.id, r.profile.counter).await?;
        tokio::try_join!(
            r.side(true, &sizes, anchor, U256::from(rx)),
            r.side(false, &sizes, anchor, U256::from(ry))
        )?
    } else {
        (Vec::new(), Vec::new())
    };
    r.profile.current(ctx)?;
    Ok(PoolState::OraclePmm(Box::new(OraclePmmData {
        price: anchor,
        spread: U256::ZERO,
        coeff: U256::ZERO,
        feasible: !buy.is_empty() || !sell.is_empty(),
        paused: !active,
        implementation: r.profile.implementation,
        base_is_token0: ordered.0 == r.profile.bases[0],
        ladder_quote_in: buy,
        ladder_base_in: sell,
        pool_token0: Some(Address::ZERO),
    })))
}
