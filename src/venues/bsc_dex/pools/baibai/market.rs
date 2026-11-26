//! Строит независимую глубину сторон WETH/USDC по чистым котировкам конкретного плательщика.
use super::{BOOK, ENTRY, ENTRY_IMPL, IBai, VAULT, elfomo, lunar, verify_at, view};
use crate::core::types::{LadderPoint, OraclePmmData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    math::full_math::mul_div,
    pools::{families::Family, ladder_counter, registry::Registry},
    tokens::Tokens,
    wallet::signer,
};
use alloy_primitives::{Address, U256};
use serde_json::Value;

struct Market<'a> {
    ctx: &'a Ctx,
    tag: &'a Value,
    payer: Address,
}

impl Market<'_> {
    async fn reserve(&self, asset: Address) -> GatewayResult<U256> {
        view(
            self.ctx,
            self.tag,
            self.payer,
            asset,
            IBai::balanceOfCall { account: VAULT },
        )
        .await
    }

    async fn net(&self, asset: Address, size: U256) -> GatewayResult<U256> {
        // Эта функция размещённого Entry учитывает fee taker, обе границы срока и оставшуюся глубину.
        let result = view(
            self.ctx,
            self.tag,
            self.payer,
            ENTRY,
            IBai::quoteForCall {
                base: lunar::WETH,
                tokenIn: asset,
                amountIn: size,
                taker: self.payer,
            },
        )
        .await?;
        Ok(result.amountOut)
    }

    async fn side(
        &self,
        asset: Address,
        sizes: &[U256],
        mid: U256,
        reserve: U256,
    ) -> GatewayResult<Vec<LadderPoint>> {
        let mut points: Vec<LadderPoint> = Vec::new();
        for &size in sizes {
            let dx = if asset == lunar::USDC {
                size
            } else {
                // mid — сырые USDC за 10^18 WETH, только для выбора входного объёма.
                let Some(coins) = mul_div(size, U256::from(10u64.pow(18)), mid) else {
                    break;
                };
                coins
            };
            if dx.is_zero() {
                continue;
            }
            let out = self.net(asset, dx).await?;
            if out.is_zero() || out > reserve {
                break;
            }
            if points
                .last()
                .is_some_and(|p| dx <= p.amount_in || out <= p.amount_out)
            {
                continue;
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
    if !(lunar::pair(from, to) || lunar::pair(to, from)) {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Пара BaiBai не прошла архивную проверку",
        ));
    }
    let tag = elfomo::block(ctx, at).await?;
    verify_at(ctx, id, &tag).await?;
    let m = Market {
        ctx,
        tag: &tag,
        payer: signer::address(ctx)?,
    };
    let (out, reserve) = tokio::try_join!(m.net(from, dx), m.reserve(to))?;
    if out.is_zero() || out > reserve {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "У BaiBai истёк срок цены, исчерпана глубина или резерв",
        ));
    }
    Ok(out)
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
    if reference != ENTRY
        || !(lunar::pair(ordered.0, ordered.1) || lunar::pair(ordered.1, ordered.0))
    {
        return Err(ctx.err(ErrorCode::NotApplicable, "Нет подтверждения пары BaiBai"));
    }
    let tag = elfomo::block(ctx, at).await?;
    verify_at(ctx, f.id, &tag).await?;
    let m = Market {
        ctx,
        tag: &tag,
        payer: signer::address(ctx)?,
    };
    let (p, base, usd) = tokio::try_join!(
        view(
            ctx,
            &tag,
            m.payer,
            BOOK,
            IBai::pairCall { base: lunar::WETH }
        ),
        m.reserve(lunar::WETH),
        m.reserve(lunar::USDC),
    )?;
    let mid = U256::from(p.mid);
    let sizes = ladder_counter::amounts(ctx, tokens, reg, f.id, lunar::USDC).await?;
    let (buy, sell) = if p.qUnit.is_zero() || mid.is_zero() {
        (Vec::new(), Vec::new())
    } else {
        tokio::try_join!(
            m.side(lunar::USDC, &sizes, mid, base),
            m.side(lunar::WETH, &sizes, mid, usd)
        )?
    };
    Ok(PoolState::OraclePmm(Box::new(OraclePmmData {
        price: mid,
        spread: U256::ZERO,
        coeff: U256::ZERO,
        feasible: !buy.is_empty() || !sell.is_empty(),
        paused: false,
        implementation: ENTRY_IMPL,
        base_is_token0: ordered.0 == lunar::WETH,
        pool_token0: None,
        ladder_quote_in: buy,
        ladder_base_in: sell,
    })))
}
