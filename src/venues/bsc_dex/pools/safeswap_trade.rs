//! Роли обновляемых SafeSwap, реестр пары и плательщик защищённого модуля на одном хеше.
use super::{
    abi_check::IPoolImmutables as Pool, abi_v2v3::IV2Factory as Factory, families::Family,
    nami_batch as batch, solidly_classic_factory as code,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc, wallet::signer},
};
use alloy::sol;
use alloy_primitives::{Address, B256, keccak256};
use serde_json::{Value, json};

sol! {interface ISafeDeployment{
    function factory() external view returns(address);
    function router() external view returns(address);
    function swapRouter() external view returns(address);
    function routerTrade() external view returns(address);
    function trade() external view returns(address);
    function wrapped() external view returns(address);
    function WETH() external view returns(address);
    function executor() external view returns(address);
    function payer() external view returns(address);
    function version() external view returns(uint256);
    function impls(uint256 index) external view returns(address);
    function implementation() external view returns(address);
    function whitelistAccess(address user) external view returns(bool);
    function quote(address input,address output,uint256 amount,bool separate) external view returns(uint256 net,uint256 fee);
}}
pub(in crate::venues::bsc_dex) fn bad(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "SafeSwap Trade: изменились реестр, код, плательщик или разрешённый роутер",
    )
}
pub(super) fn hash(ctx: &Ctx, id: &str, key: &str) -> GatewayResult<B256> {
    ctx.dex_text(key, id)?
        .parse::<B256>()
        .ok()
        .filter(|h| !h.is_zero() && *h != keccak256([]))
        .ok_or_else(|| bad(ctx))
}
struct Roles {
    factory: Address,
    public: Address,
    spender: Address,
    module: Address,
    ips: [Address; 5],
    hashes: [B256; 5],
}
impl Roles {
    fn read(ctx: &Ctx, f: &Family) -> GatewayResult<Self> {
        fn list<T: std::str::FromStr>(ctx: &Ctx, id: &str, key: &str) -> GatewayResult<[T; 5]> {
            ctx.dex_list(key, id)?
                .iter()
                .map(|s| s.parse().map_err(|_| bad(ctx)))
                .collect::<GatewayResult<Vec<_>>>()?
                .try_into()
                .map_err(|_| bad(ctx))
        }
        let ips: [Address; 5] = list(ctx, f.id, "dex.proxy_implementations")?;
        let hashes: [B256; 5] = list(ctx, f.id, "dex.proxy_runtime_hashes")?;
        if ips
            .iter()
            .enumerate()
            .any(|(i, a)| a.is_zero() || ips[..i].contains(a))
            || hashes.iter().any(|h| h.is_zero() || *h == keccak256([]))
        {
            return Err(bad(ctx));
        }
        Ok(Self {
            factory: ctx.dex_addr("dex.factory", f.id)?,
            public: ctx.dex_addr("dex.quoter", f.id)?,
            spender: ctx.dex_addr("dex.safeswap_spender", f.id)?,
            module: ctx.dex_addr("dex.router", f.id)?,
            ips,
            hashes,
        })
    }
    async fn code(&self, ctx: &Ctx, id: &str, pool: Address, at: &Value) -> GatewayResult<()> {
        let bound = |n: usize| Some((self.ips[n], self.hashes[n]));
        let ((), (), (), (), (), (), ()) = tokio::try_join!(
            code::bind(
                ctx,
                self.factory,
                hash(ctx, id, "dex.factory_runtime_hash")?,
                bound(0),
                at
            ),
            code::bind(
                ctx,
                self.spender,
                hash(ctx, id, "dex.router_runtime_hash")?,
                bound(1),
                at
            ),
            code::bind(
                ctx,
                self.public,
                hash(ctx, id, "dex.quoter_runtime_hash")?,
                bound(2),
                at
            ),
            code::plain(ctx, self.ips[3], self.hashes[3], at),
            code::plain(ctx, self.ips[4], self.hashes[4], at),
            code::runtime(ctx, pool, hash(ctx, id, "dex.pair_runtime_hash")?, at),
            super::direct_module::verify_code(ctx, id, self.module, at)
        )?;
        Ok(())
    }
}
pub(in crate::venues::bsc_dex) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    (a, b): (Address, Address),
    at: &Value,
) -> GatewayResult<Address> {
    if f.variant==super::family_variant::Variant::DegenSwap {
        return Box::pin(super::degenswap::market(ctx,f,pool,(a,b),at)).await;
    }
    if ctx.chain_id() != Some(1) {
        return Err(bad(ctx));
    }
    let generation = ctx.settings.generation();
    let r = Roles::read(ctx, f)?;
    let init = ctx.dex_list("dex.pool_code_hashes", f.id)?;
    let [init] = init.as_slice() else {
        return Err(bad(ctx));
    };
    let init = init.parse::<B256>().map_err(|_| bad(ctx))?;
    if a.is_zero()
        || a >= b
        || init.is_zero()
        || super::verify::v2_create2(r.factory, a, b, init) != pool
    {
        return Err(bad(ctx));
    }
    // У пары UpgradeableProxy читает адрес фабрики из этого слота и вызывает factory.implementation().
    // SafeswapFactory.sol, строки 991 и 996–1005 (в оболочке самой фабрики другой слот):
    // https://etherscan.io/address/0x511406a39a074782b8e4ec6d4a77ac6466970b14#code
    // https://sourcify.dev/server/v2/contract/1/0x511406a39a074782b8e4ec6d4a77ac6466970b14?fields=sources
    let slot: B256 = "0xb2101b231486a8a17a16c101f8dde1145d21799358462f57035a227f25614da3"
        .parse()
        .expect("static storage key");
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let listed = Factory::getPairCall {
        tokenA: a,
        tokenB: b,
    };
    let ok = ISafeDeployment::whitelistAccessCall { user: r.public };
    let calls = [
        batch::view(at, r.factory, listed.clone()),
        batch::view(at, pool, Pool::factoryCall {}),
        batch::view(at, r.factory, ISafeDeployment::routerCall {}),
        batch::view(at, r.factory, ISafeDeployment::implementationCall {}),
        batch::view(at, r.spender, ISafeDeployment::factoryCall {}),
        batch::view(at, r.spender, ISafeDeployment::WETHCall {}),
        batch::view(at, r.spender, ISafeDeployment::routerTradeCall {}),
        batch::view(at, r.public, ISafeDeployment::swapRouterCall {}),
        batch::view(at, r.spender, ok.clone()),
        batch::view(at, r.spender, ISafeDeployment::versionCall {}),
        batch::view(at, r.module, ISafeDeployment::factoryCall {}),
        batch::view(at, r.module, ISafeDeployment::routerCall {}),
        batch::view(at, r.module, ISafeDeployment::tradeCall {}),
        batch::view(at, r.module, ISafeDeployment::wrappedCall {}),
        batch::view(at, r.module, ISafeDeployment::payerCall {}),
        batch::view(at, r.module, ISafeDeployment::executorCall {}),
        batch::view(at, pool, Pool::token0Call {}),
        batch::view(at, pool, Pool::token1Call {}),
    ];
    let (_, factory_slot, rows) = tokio::try_join!(
        r.code(ctx, f.id, pool, at),
        rpc::call::<B256>(ctx, "eth_getStorageAt", json!([pool, slot, at])),
        batch::read(ctx, calls)
    )?;
    let addr = |i: usize| batch::decode(&rows[i], ISafeDeployment::factoryCall {});
    let version = batch::decode(&rows[9], ISafeDeployment::versionCall {})?;
    let query = ISafeDeployment::implsCall { index: version };
    let [engine] = batch::read(ctx, [batch::view(at, r.spender, query.clone())]).await?;
    let role_indices = [0, 1, 2, 3, 4, 5, 6, 7, 10, 11, 12, 13, 14, 16, 17];
    let required = [
        pool,
        r.factory,
        r.spender,
        r.ips[4],
        r.factory,
        wrapped,
        r.public,
        r.spender,
        r.factory,
        r.spender,
        r.public,
        wrapped,
        signer::address(ctx)?,
        a,
        b,
    ];
    for (i, wanted) in role_indices.into_iter().zip(required) {
        if addr(i)? != wanted {
            return Err(bad(ctx));
        }
    }
    if factory_slot != r.factory.into_word()
        || batch::decode(&engine, query)? != r.ips[3]
        || !batch::decode(&rows[8], ok)?
        || ctx.executor()?.is_some_and(|x| addr(15).ok() != Some(x))
        || ctx.settings.generation() != generation
    {
        return Err(bad(ctx));
    }
    Ok(r.module)
}
