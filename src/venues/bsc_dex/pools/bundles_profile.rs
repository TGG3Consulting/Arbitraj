//! Профиль Bundles Swap и чтение общих условий обмена на одном каноническом блоке.
use super::super::{ctx::Ctx, math::bundles::Market};
use super::{
    antfarm::IFarm, deployment_at_head::Cache, families::Family, nami_batch as batch,
    solidly_classic_factory,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::Value;
use std::sync::LazyLock;

sol! { interface IBundle {
    function getAmountsOut(uint256 amount,address[] path,uint16[] fees) external view returns(uint256[]);
    function estimateExactETHForTokens(address output,uint16 fee,uint256 amount) external view returns(uint256);
    function estimateExactTokensForETH(address input,uint16 fee,uint256 amount) external view returns(uint256);
    function protocolToken() external view returns(address);
    function protocolTokenReserve() external view returns(uint256);
    function oraclePair() external view returns(address);
    function pairOracle() external view returns(address);
    function totalDeposits() external view returns(uint256);
    function weth() external view returns(address);
} }
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Profile {
    pub factory: Address,
    pub router: Address,
    pub weth: Address,
    pub factory_hash: B256,
    pub router_hash: B256,
    pub init: [B256; 2],
    pub code: [B256; 2],
}
fn error(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotConfigured,
        "Bundles Swap: нужны проверенные коды фабрики, Router и двух видов пар",
    )
}
impl Profile {
    pub fn read(ctx: &Ctx, f: &Family) -> GatewayResult<Self> {
        let parse = |s: &str| {
            s.parse::<B256>()
                .ok()
                .filter(|h| !h.is_zero() && *h != keccak256([]))
                .ok_or_else(|| error(ctx))
        };
        let both = |key| -> GatewayResult<_> {
            let vals = ctx.dex_list(key, f.id)?;
            let [a, b] = vals.as_slice() else {
                return Err(error(ctx));
            };
            let pair = [parse(a)?, parse(b)?];
            (pair[0] != pair[1])
                .then_some(pair)
                .ok_or_else(|| error(ctx))
        };
        Ok(Self {
            factory: ctx.dex_addr("dex.factory", f.id)?,
            router: ctx.dex_addr("dex.quoter", f.id)?,
            weth: ctx.addr("dex.native_wrapped")?,
            factory_hash: parse(&ctx.dex_text("dex.factory_runtime_hash", f.id)?)?,
            router_hash: parse(&ctx.dex_text("dex.router_runtime_hash", f.id)?)?,
            init: both("dex.pool_code_hashes")?,
            code: both("dex.bundles_pair_runtime_hashes")?,
        })
    }
    pub fn derived(self, tokens: [Address; 2], fee: u16, bun: Address) -> Address {
        let mut salt = [0u8; 62];
        salt[..20].copy_from_slice(tokens[0].as_slice());
        salt[20..40].copy_from_slice(tokens[1].as_slice());
        salt[40..42].copy_from_slice(&fee.to_be_bytes());
        salt[42..].copy_from_slice(bun.as_slice());
        self.factory
            .create2(keccak256(salt), self.init[usize::from(tokens[0] == bun)])
    }
}
#[derive(Clone)]
pub(super) struct Common {
    pub bun: Address,
    pub fee_pair: Market,
    pub wallet_balance: U256,
    pub deposits: U256,
}
type CanonicalCache = Cache<Common, (B256, Profile)>;
static COMMON: LazyLock<CanonicalCache> = LazyLock::new(Default::default);
pub(super) async fn common(ctx: &Ctx, profile: Profile, at: &Value) -> GatewayResult<Common> {
    let head = at["blockHash"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .filter(|_: &B256| at["requireCanonical"] == true)
        .ok_or_else(|| error(ctx))?;
    COMMON
        .at((head, profile))
        .get_or_try_init(|| read(ctx, profile, at))
        .await
        .cloned()
}
async fn read(ctx: &Ctx, p: Profile, at: &Value) -> GatewayResult<Common> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Bundles Swap: разошлись роли фабрики, BUN или оракульной пары",
        )
    };
    let roles = batch::read(
        ctx,
        [
            batch::view(at, p.factory, IBundle::protocolTokenCall {}),
            batch::view(at, p.router, IBundle::protocolTokenCall {}),
            batch::view(at, p.router, IFarm::factoryCall {}),
            batch::view(at, p.router, IBundle::wethCall {}),
            batch::view(at, p.router, IBundle::oraclePairCall {}),
            batch::view(at, p.router, IBundle::totalDepositsCall {}),
        ],
    );
    let ((), (), [fp, rp, f, w, o, d]) = tokio::try_join!(
        solidly_classic_factory::runtime(ctx, p.factory, p.factory_hash, at),
        solidly_classic_factory::runtime(ctx, p.router, p.router_hash, at),
        roles
    )?;
    let bun = batch::decode(&fp, IBundle::protocolTokenCall {})?;
    let oracle = batch::decode(&o, IBundle::oraclePairCall {})?;
    if bun.is_zero()
        || bun == p.weth
        || batch::decode(&rp, IBundle::protocolTokenCall {})? != bun
        || batch::decode(&f, IFarm::factoryCall {})? != p.factory
        || batch::decode(&w, IBundle::wethCall {})? != p.weth
        || oracle != p.derived([bun, p.weth], 10, bun)
    {
        return Err(bad());
    }
    let bal = IFarm::balanceOfCall { account: p.router };
    let (fee_pair, [v]) = tokio::try_join!(
        super::bundles::market(ctx, p, bun, oracle, (bun, p.weth), at),
        batch::read(ctx, [batch::view(at, bun, bal.clone())])
    )?;
    Ok(Common {
        bun,
        fee_pair,
        wallet_balance: batch::decode(&v, bal)?,
        deposits: batch::decode(&d, IBundle::totalDepositsCall {})?,
    })
}
