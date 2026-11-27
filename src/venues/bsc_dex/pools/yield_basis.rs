//! Canonical YieldBasis factory tuple, deployed LEVAMM generations and physical pool addresses.
use super::{
    balancer_v1::{data, views},
    families::Family,
    solidly_classic_factory,
};
use crate::{
    core::types::yield_basis::{LevState, Limits},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{abi::IERC20, call, rpc},
    },
};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};

alloy::sol! {
    interface IYieldBasis {
        function markets(uint256 index) external view returns(address asset,address cryptopool,address amm,address lt,address oracle,address virtual_pool,address staker);
        event MarketParameters(uint256 indexed idx,address indexed asset_token,address indexed cryptopool,address amm,address lt,address oracle,address virtual_pool,address staker,address agg);
        function market_count() external view returns(uint256);
        function coins(uint256 i) external view returns(address);
        function get_dy(uint256 i,uint256 j,uint256 qty) external view returns(uint256);
        function get_state() external view returns(uint256 collateral,uint256 debt,uint256 x0);
        function fee() external view returns(uint256);
        function is_killed() external view returns(bool);
        function price() external view returns(uint256);
        function decimals() external view returns(uint8);
        function LEVERAGE() external view returns(uint256);
        function STABLECOIN() external view returns(address);
        function PRICE_ORACLE_CONTRACT() external view returns(address);
        function LT_CONTRACT() external view returns(address);
        function AMM() external view returns(address);
        function POOL() external view returns(address);
        function FACTORY() external view returns(address);
    }
}
pub(crate) fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "YieldBasis: блок, код, состояние или реестр рынка не совпали",
    )
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct YieldBasisMarket {
    pub index: u64,
    pub role: u8,
    pub venue: Address,
    pub stable: Address,
    pub cryptopool: Address,
    pub amm: Address,
    pub asset: Address,
    pub oracle: Address,
    pub lt: Address,
    pub virtual_pool: Address,
    limits: Option<Limits>,
}
impl YieldBasisMarket {
    pub fn coins(&self) -> [Address; 2] {
        [
            self.stable,
            if self.role == 1 {
                self.cryptopool
            } else {
                self.asset
            },
        ]
    }
    pub fn id(&self) -> B256 {
        let mut bytes = [0u8; 32];
        bytes[3..11].copy_from_slice(&self.index.to_be_bytes());
        bytes[11] = self.role;
        bytes[12..].copy_from_slice(self.venue.as_slice());
        B256::from(bytes)
    }
}
pub(crate) fn key(id: B256) -> Option<(u64, u8, Address)> {
    let bytes = id.as_slice();
    if bytes[..3] != [0u8; 3] || bytes[11] > 2 {
        return None;
    }
    let venue = Address::from_slice(&bytes[12..]);
    (!venue.is_zero()).then_some((
        u64::from_be_bytes(bytes[3..11].try_into().ok()?),
        bytes[11],
        venue,
    ))
}
pub(crate) async fn factory(ctx: &Ctx, f: &Family, at: &Value) -> GatewayResult<Address> {
    let address = ctx.dex_addr("dex.factory", f.id)?;
    let hash = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse()
        .map_err(|_| bad(ctx))?;
    solidly_classic_factory::runtime(ctx, address, hash, at).await?;
    Ok(address)
}
pub(super) fn from_row(
    index: u64,
    role: u8,
    stable: Address,
    r: &IYieldBasis::marketsReturn,
) -> Option<YieldBasisMarket> {
    let venue = *([r.cryptopool, r.amm, r.virtual_pool].get(usize::from(role))?);
    let m = YieldBasisMarket {
        index,
        role,
        venue,
        stable,
        cryptopool: r.cryptopool,
        amm: r.amm,
        asset: r.asset,
        oracle: r.oracle,
        lt: r.lt,
        virtual_pool: r.virtual_pool,
        limits: None,
    };
    if venue.is_zero()
        || [r.asset, r.cryptopool, r.amm, r.lt, r.oracle, stable]
            .iter()
            .any(|t| t.is_zero())
        || m.coins()[0] == m.coins()[1]
    {
        return None;
    }
    Some(m)
}
async fn code_hash(ctx: &Ctx, address: Address, at: &Value) -> GatewayResult<B256> {
    let bytes: Bytes = rpc::call(ctx, "eth_getCode", json!([address, at])).await?;
    if bytes.is_empty() {
        return Err(bad(ctx));
    }
    Ok(keccak256(bytes))
}
fn version(ctx: &Ctx, f: &Family, hash: B256) -> GatewayResult<Limits> {
    let mut matched = None;
    for line in ctx.dex_list("dex.yield_basis_amm_hashes", f.id)? {
        let mut fields = line.split_whitespace();
        let parsed = fields
            .next()
            .and_then(|h| h.parse::<B256>().ok())
            .ok_or_else(|| bad(ctx))?;
        let rule = match fields.next() {
            Some("always") => Limits::Always,
            Some("relax_improving") => Limits::RelaxImproving,
            _ => return Err(bad(ctx)),
        };
        if fields.next().is_some() || (parsed == hash && matched.replace(rule).is_some()) {
            return Err(bad(ctx));
        }
    }
    matched.ok_or_else(|| bad(ctx))
}
pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    index: u64,
    role: u8,
    at: &Value,
) -> GatewayResult<YieldBasisMarket> {
    let factory = factory(ctx, f, at).await?;
    let rows = views(
        ctx,
        &[
            data(
                factory,
                IYieldBasis::marketsCall {
                    index: U256::from(index),
                },
            ),
            data(factory, IYieldBasis::STABLECOINCall {}),
        ],
        at,
    )
    .await?;
    let mut m = from_row(
        index,
        role,
        call::decode::<IYieldBasis::STABLECOINCall>(&rows[1])?,
        &call::decode::<IYieldBasis::marketsCall>(&rows[0])?,
    )
    .ok_or_else(|| bad(ctx))?;
    let (hash, coins) = tokio::try_join!(code_hash(ctx, m.venue, at), coins(ctx, m, at))?;
    if coins != m.coins() {
        return Err(bad(ctx));
    }
    if role == 1 {
        m.limits = Some(version(ctx, f, hash)?);
    } else if !ctx
        .dex_list("dex.yield_basis_pool_hashes", f.id)?
        .iter()
        .any(|s| s.trim().parse::<B256>().ok() == Some(hash))
    {
        return Err(bad(ctx));
    }
    if role == 2 {
        let calls = [
            data(m.virtual_pool, IYieldBasis::AMMCall {}),
            data(m.venue, IYieldBasis::POOLCall {}),
            data(m.venue, IYieldBasis::FACTORYCall {}),
        ];
        let r = views(ctx, &calls, at).await?;
        if call::decode::<IYieldBasis::AMMCall>(&r[0])? != m.amm
            || call::decode::<IYieldBasis::POOLCall>(&r[1])? != m.cryptopool
            || call::decode::<IYieldBasis::FACTORYCall>(&r[2])? != factory
        {
            return Err(bad(ctx));
        }
    }
    Ok(m)
}
async fn coins(ctx: &Ctx, m: YieldBasisMarket, at: &Value) -> GatewayResult<[Address; 2]> {
    let calls = [0u8, 1].map(|i| data(m.venue, IYieldBasis::coinsCall { i: U256::from(i) }));
    let r = views(ctx, &calls, at).await?;
    Ok([
        call::decode::<IYieldBasis::coinsCall>(&r[0])?,
        call::decode::<IYieldBasis::coinsCall>(&r[1])?,
    ])
}
pub(super) async fn read(ctx: &Ctx, m: YieldBasisMarket, at: &Value) -> GatewayResult<LevState> {
    let calls = [
        data(m.amm, IYieldBasis::get_stateCall {}),
        data(m.oracle, IYieldBasis::priceCall {}),
        data(m.amm, IYieldBasis::feeCall {}),
        data(m.amm, IYieldBasis::is_killedCall {}),
        data(m.amm, IYieldBasis::LEVERAGECall {}),
        data(m.cryptopool, IYieldBasis::decimalsCall {}),
        data(m.stable, IERC20::balanceOfCall { owner: m.amm }),
        data(m.cryptopool, IERC20::balanceOfCall { owner: m.amm }),
        data(m.amm, IYieldBasis::PRICE_ORACLE_CONTRACTCall {}),
        data(m.amm, IYieldBasis::LT_CONTRACTCall {}),
    ];
    let r = views(ctx, &calls, at).await?;
    let x = call::decode::<IYieldBasis::get_stateCall>(&r[0])?;
    let decimals = call::decode::<IYieldBasis::decimalsCall>(&r[5])?;
    if m.role != 1
        || decimals > 18
        || call::decode::<IYieldBasis::LEVERAGECall>(&r[4])?
            != U256::from(2_000_000_000_000_000_000u64)
        || call::decode::<IYieldBasis::PRICE_ORACLE_CONTRACTCall>(&r[8])? != m.oracle
        || call::decode::<IYieldBasis::LT_CONTRACTCall>(&r[9])? != m.lt
    {
        return Err(bad(ctx));
    }
    let s = LevState {
        amm: m.amm,
        lp: m.cryptopool,
        stable: m.stable,
        oracle: m.oracle,
        lt: m.lt,
        collateral: x.collateral,
        debt: x.debt,
        price: call::decode::<IYieldBasis::priceCall>(&r[1])?,
        precision: U256::from(10u64.pow(u32::from(18 - decimals))),
        fee: call::decode::<IYieldBasis::feeCall>(&r[2])?,
        cash: [
            call::decode::<IERC20::balanceOfCall>(&r[6])?,
            call::decode::<IERC20::balanceOfCall>(&r[7])?,
        ],
        limits: m.limits.ok_or_else(|| bad(ctx))?,
        killed: call::decode::<IYieldBasis::is_killedCall>(&r[3])?,
    };
    if s.x().and_then(|v| v.checked_add(s.debt)) != Some(x.x0) {
        return Err(bad(ctx));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::Read;
    #[test]
    fn packed_market_ids_keep_registered_assets_and_shared_curve_sites() {
        let original = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/yield_basis_ethereum.json.gz"
        );
        let mut buf = Vec::new();
        flate2::read::GzDecoder::new(original.as_slice())
            .read_to_end(&mut buf)
            .unwrap();
        let a: Value = serde_json::from_slice(&buf).unwrap();
        let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let stable = addr(&a["stable"]);
        let mut seen = HashSet::new();
        for (index, row) in a["markets"].as_array().unwrap().iter().enumerate() {
            let record = IYieldBasis::marketsReturn {
                asset: addr(&row["asset"]),
                cryptopool: addr(&row["cryptopool"]),
                amm: addr(&row["amm"]),
                lt: addr(&row["lt"]),
                oracle: addr(&row["oracle"]),
                virtual_pool: addr(&row["virtual_pool"]),
                staker: addr(&row["staker"]),
            };
            for role in 0..3 {
                let m = from_row(index as u64, role, stable, &record).unwrap();
                let expected = &a["venues"][format!("{:#x}", m.venue)];
                assert_eq!(
                    m.coins(),
                    [addr(&expected["coins"][0]), addr(&expected["coins"][1])]
                );
                assert_eq!(key(m.id()), Some((index as u64, role, m.venue)));
                seen.insert((role, m.venue));
            }
            let missing = IYieldBasis::marketsReturn {
                virtual_pool: Address::ZERO,
                ..record
            };
            assert!(from_row(index as u64, 0, stable, &missing).is_some());
            assert!(from_row(index as u64, 1, stable, &missing).is_some());
            assert!(from_row(index as u64, 2, stable, &missing).is_none());
        }
        assert_eq!(seen.len(), 30);
    }
}
