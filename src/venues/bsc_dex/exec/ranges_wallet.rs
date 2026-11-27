//! Официальный single-pool Router Puzzle Ranges. Фактические суммы связываются с Vault.Swap и кошельком.
use super::super::{
    ctx::Ctx,
    pools::{
        abi_balancer_v3::{IBalancerV3Factory as Factory, IBalancerV3Vault as Vault},
        exact_wallet_tokens,
        family_variant::Variant,
        registry::Entry,
        solidly_classic_factory::plain,
    },
    quote::{
        v2_wallet_sim::{self, Order, Wire},
        white_state::{self, Head},
    },
    wallet::approve,
};
use super::{
    chain::Step,
    plan::{DirectCall, Plan},
    registered_v3_wallet::RouterMarket,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
use serde_json::Value;

sol! { interface Router {
    function getPermit2() external view returns(address);
    function getVault() external view returns(address);
    function swapSingleTokenExactIn(address pool,address tokenIn,address tokenOut,uint256 exactAmountIn,
        uint256 minAmountOut,uint256 deadline,bool wethIsEth,bytes userData) external payable returns(uint256);
}}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Market {
    pub pool: Address,
    pub vault: Address,
    pub permit: Address,
}
pub type Proof = super::registered_v3_wallet::Proof<Market>;
impl RouterMarket for Market {
    fn pool(self) -> Address {
        self.vault
    }
    fn event(self) -> (&'static str, usize) {
        (
            "Swap(address,address,address,uint256,uint256,uint256,uint256)",
            128,
        )
    }
    fn topics(
        self,
        actual: &[B256],
        sig: B256,
        _router: Address,
        _payer: Address,
        input: Address,
        output: Address,
    ) -> bool {
        actual
            == [
                sig,
                self.pool.into_word(),
                input.into_word(),
                output.into_word(),
            ]
    }
    fn matches(self, data: &[u8], _side: usize, spent: U256, received: U256, budget: U256) -> bool {
        spent == budget
            && U256::from_be_slice(&data[..32]) == spent
            && U256::from_be_slice(&data[32..64]) == received
    }
    fn order(
        self,
        input: Address,
        output: Address,
        amount: U256,
        minimum: U256,
        _payer: Address,
        deadline: U256,
    ) -> Bytes {
        Router::swapSingleTokenExactInCall {
            pool: self.pool,
            tokenIn: input,
            tokenOut: output,
            exactAmountIn: amount,
            minAmountOut: minimum,
            deadline,
            wethIsEth: false,
            userData: Bytes::new(),
        }
        .abi_encode()
        .into()
    }
}
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Ranges: роутер, хранилище, разрешение или пара не соответствуют поручению",
    )
}
impl Market {
    pub(in crate::venues::bsc_dex) fn read(ctx: &Ctx, e: &Entry) -> GatewayResult<(Self, Address)> {
        let id = e.family.id;
        if e.family.variant != Variant::PuzzleRanges
            || !matches!(&e.pool.state,crate::core::types::PoolState::VaultPool(d)
                if super::super::math::puzzle_ranges::active(d) && !d.paused)
        {
            return Err(bad(ctx));
        }
        Ok((
            Self {
                pool: e.pool.reference.address(),
                vault: ctx.dex_addr("dex.vault", id)?,
                permit: ctx.dex_addr("dex.permit2", id)?,
            },
            ctx.dex_addr("dex.quoter", id)?,
        ))
    }
    pub(in crate::venues::bsc_dex) fn proof(
        self,
        o: &Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        Proof {
            market: self,
            target: o.target,
            payer: o.payer,
            input: o.input,
            output: o.output,
            budget: o.amount,
            minimum,
            deadline,
        }
    }
    pub(in crate::venues::bsc_dex) async fn checked(
        self,
        ctx: &Ctx,
        e: &Entry,
        router: Address,
        tokens: (Address, Address),
        at: &Value,
    ) -> GatewayResult<()> {
        use super::super::node::call;
        let id = e.family.id;
        let factory = ctx.dex_addr("dex.factory", id)?;
        let hash = |key| ctx.dex_text(key, id)?.parse::<B256>().map_err(|_| bad(ctx));
        let pair = Factory::isPoolFromFactoryCall { pool: self.pool };
        let router_hash = hash("dex.router_runtime_hash")?;
        let permit_hash = hash("dex.permit2_runtime_hash")?;
        let (vault, permit, registered, assets, (), (), ()) = tokio::try_join!(
            call::canonical(ctx, at, router, Router::getVaultCall {}),
            call::canonical(ctx, at, router, Router::getPermit2Call {}),
            call::canonical(ctx, at, factory, pair),
            call::canonical(
                ctx,
                at,
                self.vault,
                Vault::getPoolTokensCall { pool: self.pool }
            ),
            plain(ctx, router, router_hash, at),
            plain(ctx, self.permit, permit_hash, at),
            exact_wallet_tokens::check(ctx, id, tokens.0, tokens.1, at)
        )?;
        if vault != self.vault
            || permit != self.permit
            || !registered
            || tokens.0 == tokens.1
            || !assets.contains(&tokens.0)
            || !assets.contains(&tokens.1)
        {
            return Err(bad(ctx));
        }
        Ok(())
    }
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    wallet: Address,
    height: u64,
    selected: Option<Address>,
) -> GatewayResult<Plan> {
    let generation = ctx.settings.generation();
    let (m, target) = Market::read(ctx, &s.entry)?;
    let o = &s.order;
    if selected != Some(target) || !s.from_wallet || !s.to_wallet {
        return Err(bad(ctx));
    }
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    m.checked(
        ctx,
        &s.entry,
        target,
        (o.token_in, o.token_out),
        &h.selector(),
    )
    .await?;
    let amount = o.amount_in;
    let order = Order {
        payer: wallet,
        target,
        input: o.token_in,
        output: o.token_out,
        amount,
        minimum: o.min_out,
        wire: Wire::Ranges(m),
    };
    let deadline = h
        .timestamp
        .to::<u64>()
        .checked_add(ctx.int("dex.deadline_s")?)
        .ok_or_else(|| bad(ctx))?;
    let proof = m.proof(&order, o.min_out, U256::from(deadline));
    let approvals = permissions(ctx, h, m, &order).await?;
    v2_wallet_sim::measure(ctx, h, order, true).await?;
    if ctx.settings.generation() != generation {
        return Err(bad(ctx));
    }
    let mut p = Plan::new(
        ctx,
        (approvals, target, wallet),
        o.token_out,
        Vec::new(),
        o.min_out,
        U256::from(deadline),
    )?;
    p.set_direct_market(DirectCall::Ranges(std::sync::Arc::new(proof)));
    Ok(p)
}
pub(in crate::venues::bsc_dex) async fn permissions(
    ctx: &Ctx,
    h: Head,
    m: Market,
    o: &Order,
) -> GatewayResult<Vec<approve::Approval>> {
    approve::at_block(ctx, o.input, o.payer, m.permit, o.target, o.amount, h).await
}
