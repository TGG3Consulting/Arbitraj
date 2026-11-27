//! Выбор и оплата официального Omni Flux Router. Один канонический блок связывает
//! код получателя разрешения, адреса менеджера/Vault, исходный PoolId и исполнимую сумму.
use super::super::{
    ctx::Ctx,
    node::{abi::IERC20, call, rpc},
    pools::{exact_wallet_tokens, flux_native, registry::Entry},
    quote::{
        v2_wallet_sim::{self, Order, Wire},
        white_state::{self, Head},
    },
    wallet::approve::{self, Approval, IPermit2},
};
use super::{
    chain::Step,
    flux_eth::Market,
    plan::{DirectCall, Plan},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};
alloy::sol! {interface IRouter {
    function clPoolManager() external view returns(address);
    function binPoolManager() external view returns(address);
    function vault() external view returns(address);
    function paused() external view returns(bool);
}}
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Omni Flux: нативное поручение, код или разрешение не подтверждены",
    )
}
async fn view<C: SolCall>(
    ctx: &Ctx,
    at: &Value,
    target: Address,
    c: C,
) -> GatewayResult<C::Return> {
    let bytes: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":target,"data":Bytes::from(c.abi_encode())},at]),
    )
    .await?;
    call::decode::<C>(&bytes)
}
async fn code(ctx: &Ctx, id: &str, target: Address, key: &str, at: &Value) -> GatewayResult<()> {
    let hash: B256 = ctx.dex_text(key, id)?.parse().map_err(|_| bad(ctx))?;
    let raw: Bytes = rpc::call(ctx, "eth_getCode", json!([target, at])).await?;
    if raw.is_empty() || keccak256(raw) != hash {
        return Err(bad(ctx));
    }
    Ok(())
}
pub(in crate::venues::bsc_dex) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    at: &Value,
) -> GatewayResult<Market> {
    let generation = ctx.settings.generation();
    let m = Market::read(ctx, e)?;
    let f = e.family;
    let check_manager = async {
        if m.bin {
            view(ctx, at, m.router, IRouter::binPoolManagerCall {}).await
        } else {
            view(ctx, at, m.router, IRouter::clPoolManagerCall {}).await
        }
    };
    let (found, (), (), (), manager, vault, paused) = tokio::try_join!(
        flux_native::market(ctx, f, m.manager, m.id, (m.wrapped, m.token), at),
        exact_wallet_tokens::check_input(ctx, f.id, m.token, at),
        code(ctx, f.id, m.router, "dex.router_runtime_hash", at),
        code(ctx, f.id, m.permit, "dex.permit2_runtime_hash", at),
        check_manager,
        view(ctx, at, m.router, IRouter::vaultCall {}),
        view(ctx, at, m.router, IRouter::pausedCall {})
    )?;
    if found.key != e.key
        || manager != m.manager
        || vault != ctx.dex_addr("dex.vault", f.id)?
        || paused
        || ctx.settings.generation() != generation
    {
        return Err(bad(ctx));
    }
    Ok(m)
}
pub(in crate::venues::bsc_dex) async fn approvals(
    ctx: &Ctx,
    h: Head,
    m: Market,
    o: &Order,
) -> GatewayResult<Vec<Approval>> {
    if o.target != m.router {
        return Err(bad(ctx));
    }
    if o.input == m.wrapped && o.output == m.token {
        return Ok(Vec::new());
    }
    if (o.input, o.output) != (m.token, m.wrapped) {
        return Err(bad(ctx));
    }
    let at = h.selector();
    let (erc20, p) = tokio::try_join!(
        view(
            ctx,
            &at,
            m.token,
            IERC20::allowanceCall {
                owner: o.payer,
                spender: m.permit
            }
        ),
        view(
            ctx,
            &at,
            m.permit,
            IPermit2::allowanceCall {
                user: o.payer,
                token: m.token,
                spender: m.router
            }
        )
    )?;
    let r = approve::Read {
        erc20,
        permit2: U256::from(p.amount),
        expiration: p.expiration.to(),
    };
    approve::lacking(
        ctx,
        &r,
        (m.token, m.permit, m.router),
        o.amount,
        h.timestamp.to(),
    )
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    selected: Option<Address>,
) -> GatewayResult<Plan> {
    if !s.from_wallet || !s.to_wallet {
        return Err(bad(ctx));
    }
    let generation = ctx.settings.generation();
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    let m = checked(ctx, &s.entry, &h.selector()).await?;
    if selected != Some(m.router) {
        return Err(bad(ctx));
    }
    let x = &s.order;
    let o = Order {
        payer,
        target: m.router,
        input: x.token_in,
        output: x.token_out,
        amount: x.amount_in,
        minimum: x.min_out,
        wire: Wire::FluxEth(m),
    };
    let deadline = h
        .timestamp
        .to::<u64>()
        .checked_add(ctx.int("dex.deadline_s")?)
        .ok_or_else(|| bad(ctx))?;
    let proof = m.proof(&o, o.minimum, U256::from(deadline));
    let auth = approvals(ctx, h, m, &o).await?;
    v2_wallet_sim::measure(ctx, h, o, true).await?;
    if generation != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    let mut p = Plan::new(
        ctx,
        (auth, m.router, payer),
        proof.output,
        Vec::new(),
        proof.minimum,
        proof.deadline,
    )?;
    p.set_direct_market(DirectCall::FluxEth(std::sync::Arc::new(proof)));
    Ok(p)
}
