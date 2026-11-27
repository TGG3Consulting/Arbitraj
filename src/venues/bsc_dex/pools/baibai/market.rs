//! Строит независимую глубину рынков WETH и NVDAc с USDC по чистым котировкам конкретного плательщика.
use super::{IBai, Profile, Proto, elfomo, verify_at, view};
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
    coin: Address,
    profile: Profile,
}

impl<'a> Market<'a> {
    async fn new(
        ctx: &'a Ctx,
        id: &str,
        tag: &'a Value,
        coin: Address,
        profile: Profile,
    ) -> GatewayResult<Self> {
        verify_at(ctx, &profile, tag).await?;
        let payer = if ctx.dex_text("dex.kind", id)? == "baibai_module" {
            let module = ctx.dex_addr("dex.router", id)?;
            super::super::direct_module::verify_code(ctx, id, module, tag).await?;
            module
        } else {
            signer::address(ctx)?
        };
        Ok(Self {
            ctx,
            tag,
            payer,
            coin,
            profile,
        })
    }
    async fn reserve(&self, asset: Address) -> GatewayResult<U256> {
        view(
            self.ctx,
            self.tag,
            self.payer,
            asset,
            IBai::balanceOfCall {
                account: self.profile.vault,
            },
        )
        .await
    }

    async fn net(&self, asset: Address, size: U256) -> GatewayResult<U256> {
        // Эта функция размещённого Entry учитывает fee taker, обе границы срока и оставшуюся глубину.
        let result = view(
            self.ctx,
            self.tag,
            self.payer,
            self.profile.pool,
            IBai::quoteForCall {
                base: self.coin,
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
            let dx = if asset == self.profile.counter {
                size
            } else {
                // mid — сырые USDC за 10^18 сырых единиц базового токена, только для выбора входного объёма.
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
    let profile = Profile::read(ctx, id, Proto::BaiBai)?;
    if !profile.unordered(from, to) {
        return Err(ctx.err(ErrorCode::NoRoute, "Этой пары нет в рабочем профиле BaiBai"));
    }
    let tag = elfomo::block(ctx, at).await?;
    let coin = if from == profile.counter { to } else { from };
    let m = Market::new(ctx, id, &tag, coin, profile).await?;
    let (out, reserve) = tokio::try_join!(m.net(from, dx), m.reserve(to))?;
    if out.is_zero() || out > reserve {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "У BaiBai истёк срок цены, исчерпана глубина или резерв",
        ));
    }
    m.profile.current(ctx)?;
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
    let profile = Profile::read(ctx, f.id, Proto::BaiBai)?;
    if reference != profile.pool || !profile.unordered(ordered.0, ordered.1) {
        return Err(ctx.err(ErrorCode::NotApplicable, "Нет подтверждения пары BaiBai"));
    }
    let tag = elfomo::block(ctx, at).await?;
    let coin = if ordered.0 == profile.counter {
        ordered.1
    } else {
        ordered.0
    };
    let m = Market::new(ctx, f.id, &tag, coin, profile).await?;
    let (p, base, usd) = tokio::try_join!(
        view(
            ctx,
            &tag,
            m.payer,
            m.profile.oracle,
            IBai::pairCall { base: m.coin }
        ),
        m.reserve(m.coin),
        m.reserve(m.profile.counter),
    )?;
    let mid = U256::from(p.mid);
    let sizes = ladder_counter::amounts(ctx, tokens, reg, f.id, m.profile.counter).await?;
    let (buy, sell) = if p.qUnit.is_zero() || mid.is_zero() {
        (Vec::new(), Vec::new())
    } else {
        tokio::try_join!(
            m.side(m.profile.counter, &sizes, mid, base),
            m.side(m.coin, &sizes, mid, usd)
        )?
    };
    m.profile.current(ctx)?;
    Ok(PoolState::OraclePmm(Box::new(OraclePmmData {
        price: mid,
        spread: U256::ZERO,
        coeff: U256::ZERO,
        feasible: !buy.is_empty() || !sell.is_empty(),
        paused: false,
        implementation: m.profile.implementation,
        base_is_token0: ordered.0 == m.coin,
        shared_accounts: Vec::new(),
        pool_token0: None,
        ladder_quote_in: buy,
        ladder_base_in: sell,
    })))
}
