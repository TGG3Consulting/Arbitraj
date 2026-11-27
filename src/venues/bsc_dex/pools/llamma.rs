//! Authentic Curve LLAMMA registry row, Vyper code prefix and physical currencies on one canonical block.
//! ABI: https://docs.curve.finance/developer/integration/llamma/ and the deployed factory ABIs linked by the recorder.
//! Directory checks and final execution use the same binding.
use super::{
    balancer_v1::{data, views},
    families::Family,
    family_variant::Variant,
    registry::Entry,
};
use crate::{
    core::types::{PoolRef, PoolState},
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{call, rpc},
    },
};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};

#[allow(clippy::too_many_arguments)]
mod abi {
    alloy::sol! {
        interface ILlama {
            function n_collaterals() external view returns(uint256);
            function market_count() external view returns(uint256);
            function markets(uint256 i) external view returns(address vault,address controller,address amm,address collateral,address borrowed,address oracle,address policy);
            function amms(uint256 i) external view returns(address);
            function controllers(uint256 i) external view returns(address);
            function collaterals(uint256 i) external view returns(address);
            function collateral_tokens(uint256 i) external view returns(address);
            function borrowed_tokens(uint256 i) external view returns(address);
            function stablecoin() external view returns(address);
            function coins(uint256 i) external view returns(address);
            function admin() external view returns(address);
            function amm() external view returns(address);
            function get_dxdy(uint256 i,uint256 j,uint256 qty) external view returns(uint256 spent,uint256 received);
            function get_dydx(uint256 i,uint256 j,uint256 qty) external view returns(uint256 received,uint256 spent);
            event AddMarket(address indexed collateral,address controller,address amm,address monetary_policy,uint256 ix);
            event NewVault(uint256 indexed id,address indexed collateral_token,address indexed borrowed_token,address vault,address controller,address amm,address price_oracle,address monetary_policy);
            event TokenExchange(address indexed recipient,uint256 sold_id,uint256 tokens_sold,uint256 bought_id,uint256 tokens_bought);
        }
    }
}
pub(crate) use abi::ILlama;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Registry {
    Lending,
    Mint,
    V2,
}
#[derive(Debug, Clone, Copy)]
pub(super) struct Row {
    pub index: u64,
    pub amm: Address,
    pub controller: Address,
    pub coins: [Address; 2],
}
pub(super) fn bad(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "LLAMMA: код, оригинальный рынок, валюта или сумма не подтверждены",
    )
}
pub(super) fn key(index: u64, amm: Address) -> B256 {
    let mut value = [0u8; 32];
    value[4..12].copy_from_slice(&index.to_be_bytes());
    value[12..].copy_from_slice(amm.as_slice());
    B256::from(value)
}
pub(crate) fn decode_key(id: B256) -> Option<(u64, Address)> {
    let b = id.as_slice();
    let amm = Address::from_slice(&b[12..]);
    (b[..4] == [0; 4] && !amm.is_zero())
        .then_some((u64::from_be_bytes(b[4..12].try_into().ok()?), amm))
}
pub(super) async fn factory(ctx: &Ctx, f: &Family, at: &Value) -> GatewayResult<Address> {
    let a = ctx.dex_addr("dex.factory", f.id)?;
    let h = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse()
        .map_err(|_| bad(ctx))?;
    super::solidly_classic_factory::runtime(ctx, a, h, at).await?;
    Ok(a)
}
impl Registry {
    pub(super) fn count(self, f: Address) -> (Address, Bytes) {
        if self == Self::Mint {
            data(f, ILlama::n_collateralsCall {})
        } else {
            data(f, ILlama::market_countCall {})
        }
    }
    pub(super) fn ask(self, f: Address, index: u64) -> Vec<(Address, Bytes)> {
        let i = U256::from(index);
        match self {
            Self::V2 => vec![data(f, ILlama::marketsCall { i })],
            Self::Lending => vec![
                data(f, ILlama::ammsCall { i }),
                data(f, ILlama::controllersCall { i }),
                data(f, ILlama::borrowed_tokensCall { i }),
                data(f, ILlama::collateral_tokensCall { i }),
            ],
            Self::Mint => vec![
                data(f, ILlama::ammsCall { i }),
                data(f, ILlama::controllersCall { i }),
                data(f, ILlama::stablecoinCall {}),
                data(f, ILlama::collateralsCall { i }),
            ],
        }
    }
    pub(super) fn row(self, ctx: &Ctx, index: u64, answers: &[Bytes]) -> GatewayResult<Row> {
        let (amm, controller, coins) = match (self, answers) {
            (Self::V2, [tuple]) => {
                let v = call::decode::<ILlama::marketsCall>(tuple)?;
                (v.amm, v.controller, [v.borrowed, v.collateral])
            }
            (Self::Mint | Self::Lending, [a, b, c, d]) => (
                call::decode::<ILlama::ammsCall>(a)?,
                call::decode::<ILlama::controllersCall>(b)?,
                [
                    call::decode::<ILlama::borrowed_tokensCall>(c)?,
                    call::decode::<ILlama::collateral_tokensCall>(d)?,
                ],
            ),
            _ => return Err(bad(ctx)),
        };
        if amm.is_zero()
            || controller.is_zero()
            || coins.contains(&Address::ZERO)
            || coins[0] == coins[1]
        {
            return Err(bad(ctx));
        }
        Ok(Row {
            index,
            amm,
            controller,
            coins,
        })
    }
}
fn matches_class(code: &[u8], coins: [Address; 2], line: &str) -> Option<bool> {
    let mut parts = line.split_whitespace();
    let body = parts.next()?.parse::<usize>().ok()?;
    let tail = parts.next()?.parse::<usize>().ok()?;
    let hash = parts.next()?.parse::<B256>().ok()?;
    if parts.next().is_some()
        || body == 0
        || tail < 96
        || !tail.is_multiple_of(32)
        || hash.is_zero()
    {
        return None;
    }
    if code.len() != body.checked_add(tail)? {
        return Some(false);
    }
    Some(
        &code[body..body + 32] == coins[0].into_word().as_slice()
            && &code[body + 64..body + 96] == coins[1].into_word().as_slice()
            && keccak256(&code[..body]) == hash,
    )
}
pub(super) async fn bound(
    ctx: &Ctx,
    f: &Family,
    index: u64,
    at: &Value,
) -> GatewayResult<(Address, Row, B256)> {
    let Variant::Llamma(kind) = f.variant else {
        return Err(bad(ctx));
    };
    let fac = factory(ctx, f, at).await?;
    let rows = views(ctx, &kind.ask(fac, index), at).await?;
    let m = kind.row(ctx, index, &rows)?;
    let links = [
        data(m.amm, ILlama::adminCall {}),
        data(m.controller, ILlama::ammCall {}),
        data(m.amm, ILlama::coinsCall { i: U256::ZERO }),
        data(m.amm, ILlama::coinsCall { i: U256::from(1) }),
    ];
    let (code, links): (Bytes, _) = tokio::try_join!(
        rpc::call(ctx, "eth_getCode", json!([m.amm, at])),
        views(ctx, &links, at)
    )?;
    if call::decode::<ILlama::adminCall>(&links[0])? != m.controller
        || call::decode::<ILlama::ammCall>(&links[1])? != m.amm
        || [
            call::decode::<ILlama::coinsCall>(&links[2])?,
            call::decode::<ILlama::coinsCall>(&links[3])?,
        ] != m.coins
    {
        return Err(bad(ctx));
    }
    let mut matched = 0;
    for l in ctx.dex_list("dex.llamma_runtime_classes", f.id)? {
        if matches_class(&code, m.coins, &l).ok_or_else(|| bad(ctx))? {
            matched += 1;
        }
    }
    if matched != 1 {
        return Err(bad(ctx));
    }
    Ok((fac, m, keccak256(code)))
}
pub(crate) fn shape(e: &Entry) -> Option<&crate::core::types::llamma::State> {
    let PoolState::Llamma(s) = &e.pool.state else {
        return None;
    };
    let PoolRef::Singleton { manager, id } = e.pool.reference else {
        return None;
    };
    (e.family.variant.llamma()
        && e.pool.kind == crate::core::types::PoolKind::V2
        && decode_key(id) == Some((s.index, s.amm))
        && manager == s.amm
        && !s.factory.is_zero()
        && !s.controller.is_zero()
        && !s.runtime.is_zero()
        && s.coins[0] != s.coins[1]
        && !s.coins.contains(&Address::ZERO)
        && (s.coins == [e.pool.token0, e.pool.token1] || s.coins == [e.pool.token1, e.pool.token0]))
    .then_some(s)
}
pub(crate) async fn check(ctx: &Ctx, e: &Entry, at: &Value) -> GatewayResult<()> {
    let s = shape(e).ok_or_else(|| bad(ctx))?;
    let (fac, m, h) = bound(ctx, e.family, s.index, at).await?;
    if (fac, m.amm, m.controller, m.coins, h)
        != (s.factory, s.amm, s.controller, s.coins, s.runtime)
    {
        return Err(bad(ctx));
    }
    Ok(())
}

#[cfg(test)]
#[path = "llamma_tests.rs"]
mod tests;
