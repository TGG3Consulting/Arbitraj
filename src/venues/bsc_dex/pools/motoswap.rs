//! MotoSwapFactory, MotoSwapPair and FeeRouter. Verified deployments and public entry points:
//! https://docs.motoswap.org/dev/addresses and https://docs.motoswap.org/dev/guides/swap-via-fee-router
use super::{
    abi_check::IPoolImmutables as Pair,
    abi_v2v3::{IV2Factory as Factory, IV2Pair},
    families::Family,
    nami_batch as batch, solidly_classic_factory as code,
};
use crate::{
    core::types::MotoswapState,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::{Value, json};

alloy::sol! {interface IMoto {
    function upgradesRenounced() external view returns(bool);
    function router() external view returns(address);
    function WETH() external view returns(address);
    function registry() external view returns(address);
    function creatorRegistry() external view returns(address);
    function creatorVault() external view returns(address);
    function collector() external view returns(address);
    function beacon() external view returns(address);
    function implementation() external view returns(address);
    function pairCodeHash() external view returns(bytes32);
    function getAmountsOut(uint256 amountIn,address[] path) external view returns(uint256[]);
    function swapFeeBps() external view returns(uint256);
    function protocolFeeBps() external view returns(uint256);
    function creatorFeeBps() external view returns(uint256);
    function swapAllowed(address who) external view returns(bool);
    function isQuote(address token) external view returns(bool);
    function activeCreator(address token) external view returns(address);
    function feeOnOutput(address[] path) external view returns(bool);
    function balanceOf(address owner) external view returns(uint256);
}}
#[derive(Clone, Copy)]
struct Role {
    address: Address,
    hash: B256,
    implementation: Option<(Address, B256)>,
}
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Motoswap: не подтверждены исходные роли, реестр пары или тариф",
    )
}
fn role(ctx: &Ctx, rows: &[String], name: &str, proxy: bool) -> GatewayResult<Role> {
    let mut selected = rows
        .iter()
        .map(|s| s.split_whitespace().collect::<Vec<_>>())
        .filter(|parts| parts.first() == Some(&name));
    let row = selected.next().ok_or_else(|| bad(ctx))?;
    if selected.next().is_some() || row.len() != if proxy { 5 } else { 3 } {
        return Err(bad(ctx));
    }
    let addr = |s: &str| {
        s.parse::<Address>()
            .ok()
            .filter(|a| !a.is_zero())
            .ok_or_else(|| bad(ctx))
    };
    let hash = |s: &str| {
        s.parse::<B256>()
            .ok()
            .filter(|h| !h.is_zero() && *h != keccak256([]))
            .ok_or_else(|| bad(ctx))
    };
    Ok(Role {
        address: addr(row[1])?,
        hash: hash(row[2])?,
        implementation: if proxy {
            Some((addr(row[3])?, hash(row[4])?))
        } else {
            None
        },
    })
}
impl Role {
    async fn check(self, ctx: &Ctx, at: &Value) -> GatewayResult<()> {
        code::bind(ctx, self.address, self.hash, self.implementation, at).await
    }
}
pub(crate) async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    (a, b): (Address, Address),
    at: &Value,
) -> GatewayResult<MotoswapState> {
    let generation = ctx.settings.generation();
    let rows = ctx.dex_list("dex.motoswap_roles", f.id)?;
    let factory = role(ctx, &rows, "factory", true)?;
    let fee = role(ctx, &rows, "fee_router", true)?;
    let core = role(ctx, &rows, "core", false)?;
    let beacon = role(ctx, &rows, "beacon", false)?;
    let pair_impl = role(ctx, &rows, "pair_impl", false)?;
    let registry = role(ctx, &rows, "registry", false)?;
    let creators = role(ctx, &rows, "creator", false)?;
    let vault = role(ctx, &rows, "vault", false)?;
    let collector = role(ctx, &rows, "collector", true)?;
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let shell = super::safeswap_trade::hash(ctx, f.id, "dex.pair_runtime_hash")?;
    let hashes = ctx.dex_list("dex.pool_code_hashes", f.id)?;
    let [seed] = hashes.as_slice() else {
        return Err(bad(ctx));
    };
    let init: B256 = seed.parse().map_err(|_| bad(ctx))?;
    if a.is_zero()
        || a >= b
        || factory.address != ctx.dex_addr("dex.factory", f.id)?
        || fee.address != ctx.dex_addr("dex.quoter", f.id)?
        || super::verify::v2_create2(factory.address, a, b, init) != pool
    {
        return Err(bad(ctx));
    }
    tokio::try_join!(
        factory.check(ctx, at),
        fee.check(ctx, at),
        core.check(ctx, at),
        beacon.check(ctx, at),
        pair_impl.check(ctx, at),
        registry.check(ctx, at),
        creators.check(ctx, at),
        vault.check(ctx, at),
        collector.check(ctx, at),
        code::runtime(ctx, pool, shell, at)
    )?;
    let fac = factory.address;
    let router = fee.address;
    let v = |to, call| batch::view(at, to, call);
    let get = |x, y| {
        batch::view(
            at,
            fac,
            Factory::getPairCall {
                tokenA: x,
                tokenB: y,
            },
        )
    };
    let abi = |t| batch::view(at, creators.address, IMoto::activeCreatorCall { token: t });
    let output = |x, y| batch::view(at, router, IMoto::feeOnOutputCall { path: vec![x, y] });
    let allowed = |who| batch::view(at, fac, IMoto::swapAllowedCall { who });
    let q = |token| batch::view(at, registry.address, IMoto::isQuoteCall { token });
    let slot = "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50";
    let [
        ab,
        ba,
        cf,
        ca,
        cb,
        pf,
        t0,
        t1,
        rw,
        fw,
        core_addr,
        reg,
        crt,
        cv,
        coll,
        fb,
        ib,
        pb,
        init_raw,
        aa,
        bb,
        qa,
        qb,
        coa,
        cob,
        fa,
        fbout,
        rr,
        lp,
        pct,
        creator,
        x,
        frozen,
    ] = batch::read(
        ctx,
        [
            get(a, b),
            get(b, a),
            batch::view(at, core.address, Pair::factoryCall {}),
            allowed(core.address),
            allowed(router),
            batch::view(at, pool, Pair::factoryCall {}),
            batch::view(at, pool, Pair::token0Call {}),
            batch::view(at, pool, Pair::token1Call {}),
            v(router, IMoto::WETHCall {}),
            v(core.address, IMoto::WETHCall {}),
            batch::view(at, router, IMoto::routerCall {}),
            batch::view(at, router, IMoto::registryCall {}),
            batch::view(at, router, IMoto::creatorRegistryCall {}),
            batch::view(at, router, IMoto::creatorVaultCall {}),
            batch::view(at, router, IMoto::collectorCall {}),
            batch::view(at, fac, IMoto::beaconCall {}),
            batch::view(at, beacon.address, IMoto::implementationCall {}),
            ("eth_getStorageAt", json!([pool, slot, at])),
            batch::view(at, fac, IMoto::pairCodeHashCall {}),
            batch::view(at, a, IMoto::balanceOfCall { owner: pool }),
            batch::view(at, b, IMoto::balanceOfCall { owner: pool }),
            q(a),
            q(b),
            abi(a),
            abi(b),
            output(a, b),
            output(b, a),
            batch::view(at, pool, IV2Pair::getReservesCall {}),
            batch::view(at, fac, IMoto::swapFeeBpsCall {}),
            batch::view(at, router, IMoto::protocolFeeBpsCall {}),
            batch::view(at, router, IMoto::creatorFeeBpsCall {}),
            batch::view(at, fac, IMoto::implementationCall {}),
            batch::view(at, router, IMoto::upgradesRenouncedCall {}),
        ],
    )
    .await?;
    let bindings = [
        (ab, pool),
        (ba, pool),
        (cf, fac),
        (pf, fac),
        (t0, a),
        (t1, b),
        (rw, wrapped),
        (fw, wrapped),
        (core_addr, core.address),
        (reg, registry.address),
        (crt, creators.address),
        (cv, vault.address),
        (coll, collector.address),
        (fb, beacon.address),
        (ib, pair_impl.address),
        (pb, beacon.address),
        (x, pair_impl.address),
    ];
    if bindings
        .iter()
        .any(|(raw, expected)| raw.as_ref() != expected.into_word().as_slice())
        || init_raw.as_ref() != init.as_slice()
        || !batch::decode(&ca, IMoto::swapAllowedCall { who: core.address })?
        || !batch::decode(&cb, IMoto::swapAllowedCall { who: router })?
        || !(batch::decode(&qa, IMoto::isQuoteCall { token: a })?
            || batch::decode(&qb, IMoto::isQuoteCall { token: b })?)
        || ctx.settings.generation() != generation
    {
        return Err(bad(ctx));
    }
    let rr = batch::decode(&rr, IV2Pair::getReservesCall {})?;
    let rate = |n: U256| u16::try_from(n).map_err(|_| bad(ctx));
    Ok(MotoswapState {
        fee_router_frozen: batch::decode(&frozen, IMoto::upgradesRenouncedCall {})?,
        reserves: [U256::from(rr.reserve0), U256::from(rr.reserve1)],
        balances: [
            batch::decode(&aa, IMoto::balanceOfCall { owner: pool })?,
            batch::decode(&bb, IMoto::balanceOfCall { owner: pool })?,
        ],
        lp_bps: rate(batch::decode(&lp, IMoto::swapFeeBpsCall {})?)?,
        protocol_bps: rate(batch::decode(&pct, IMoto::protocolFeeBpsCall {})?)?,
        creator_bps: rate(batch::decode(&creator, IMoto::creatorFeeBpsCall {})?)?,
        creators: u8::from(!batch::decode(&coa, IMoto::activeCreatorCall { token: a })?.is_zero())
            + u8::from(!batch::decode(&cob, IMoto::activeCreatorCall { token: b })?.is_zero()),
        fee_on_output: [
            batch::decode(&fa, IMoto::feeOnOutputCall { path: vec![a, b] })?,
            batch::decode(&fbout, IMoto::feeOnOutputCall { path: vec![b, a] })?,
        ],
    })
}

pub(crate) fn routing(
    ctx: &Ctx,
    id: &str,
) -> GatewayResult<(
    Address,
    Address,
    crate::venues::bsc_dex::exec::carbon_binding::Code,
)> {
    let rows = ctx.dex_list("dex.motoswap_roles", id)?;
    let fee = role(ctx, &rows, "fee_router", true)?;
    let core = role(ctx, &rows, "core", false)?;
    let (implementation, runtime) = fee.implementation.ok_or_else(|| bad(ctx))?;
    Ok((
        fee.address,
        core.address,
        crate::venues::bsc_dex::exec::carbon_binding::Code {
            shell: fee.hash,
            implementation,
            runtime,
        },
    ))
}
pub(crate) async fn frozen(ctx: &Ctx, router: Address, at: &Value) -> GatewayResult<()> {
    let call = IMoto::upgradesRenouncedCall {};
    let [raw] = batch::read(ctx, [batch::view(at, router, call)]).await?;
    if batch::decode(&raw, call)? {
        Ok(())
    } else {
        Err(bad(ctx))
    }
}

pub(crate) async fn official(
    ctx: &Ctx,
    e: &super::registry::Entry,
    input: Address,
    n: U256,
    at: &Value,
) -> GatewayResult<U256> {
    let (a, b) = (e.pool.token0, e.pool.token1);
    let i = if input == a {
        0
    } else if input == b {
        1
    } else {
        return Err(bad(ctx));
    };
    let s = Box::pin(state(ctx, e.family, e.pool.reference.address(), (a, b), at)).await?;
    let projected = s.swap(i == 0, true, n).ok_or_else(|| bad(ctx))?;
    let pair_input = if s.fee_on_output[i] {
        n
    } else {
        s.net(n).ok_or_else(|| bad(ctx))?
    };
    let effective = s.balances[i]
        .checked_add(pair_input)
        .and_then(|v| v.checked_sub(s.reserves[i]))
        .ok_or_else(|| bad(ctx))?;
    let (_, core, _) = routing(ctx, e.family.id)?;
    let out = crate::venues::bsc_dex::node::call::canonical(
        ctx,
        at,
        core,
        IMoto::getAmountsOutCall {
            amountIn: effective,
            path: vec![input, [b, a][i]],
        },
    )
    .await?;
    let [spent, gross] = out.as_slice() else {
        return Err(bad(ctx));
    };
    let net = if s.fee_on_output[i] {
        s.net(*gross).ok_or_else(|| bad(ctx))?
    } else {
        *gross
    };
    if *spent != effective || net != projected.amount_out {
        return Err(bad(ctx));
    }
    Ok(net)
}
