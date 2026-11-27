//! Состояние пары Motoswap на одном блоке: регистрация, резервы, остатки и комиссии FeeRouter.
//! Загрузчик и планировщик запрашивают это состояние перед оценкой пары и подготовкой вызова.
use super::{Ctx, Family, GatewayResult, IMoto, Pair, U256, Value, bad, batch, code, frame, role};
use crate::core::types::MotoswapState;
use crate::venues::bsc_dex::pools::{abi_v2v3::{IV2Factory as Factory, IV2Pair}, safeswap_trade, verify};
use alloy_primitives::{Address, B256};
use serde_json::json;

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
    let shell = safeswap_trade::hash(ctx, f.id, "dex.pair_runtime_hash")?;
    let hashes = ctx.dex_list("dex.pool_code_hashes", f.id)?;
    let [seed] = hashes.as_slice() else {
        return Err(bad(ctx));
    };
    let init: B256 = seed.parse().map_err(|_| bad(ctx))?;
    if a.is_zero()
        || a >= b
        || factory.address != ctx.dex_addr("dex.factory", f.id)?
        || fee.address != ctx.dex_addr("dex.quoter", f.id)?
        || verify::v2_create2(factory.address, a, b, init) != pool
    {
        return Err(bad(ctx));
    }
    let roles = frame::Roles {
        factory,
        fee,
        core,
        beacon,
        pair_impl,
        registry,
        creators,
        vault,
        collector,
        wrapped,
        init,
    };
    let (frame, ()) = tokio::try_join!(
        frame::read(ctx, roles, at),
        code::runtime(ctx, pool, shell, at)
    )?;
    let fac = factory.address;
    let router = fee.address;
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
    let q = |token| batch::view(at, registry.address, IMoto::isQuoteCall { token });
    let slot = "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50";
    let [
        ab,
        ba,
        pf,
        t0,
        t1,
        pb,
        aa,
        bb,
        qa,
        qb,
        rank0,
        rank1,
        coa,
        cob,
        fa,
        fbout,
        rr,
    ] = batch::read(
        ctx,
        [
            get(a, b),
            get(b, a),
            batch::view(at, pool, Pair::factoryCall {}),
            batch::view(at, pool, Pair::token0Call {}),
            batch::view(at, pool, Pair::token1Call {}),
            ("eth_getStorageAt", json!([pool, slot, at])),
            batch::view(at, a, IMoto::balanceOfCall { owner: pool }),
            batch::view(at, b, IMoto::balanceOfCall { owner: pool }),
            q(a),
            q(b),
            batch::view(at, registry.address, IMoto::quoteRankCall { token: a }),
            batch::view(at, registry.address, IMoto::quoteRankCall { token: b }),
            abi(a),
            abi(b),
            output(a, b),
            output(b, a),
            batch::view(at, pool, IV2Pair::getReservesCall {}),
        ],
    )
    .await?;
    if [
        (ab, pool),
        (ba, pool),
        (pf, fac),
        (t0, a),
        (t1, b),
        (pb, beacon.address),
    ]
    .iter()
    .any(|(raw, expected)| raw.as_ref() != expected.into_word().as_slice())
        || !(batch::decode(&qa, IMoto::isQuoteCall { token: a })?
            || batch::decode(&qb, IMoto::isQuoteCall { token: b })?)
        || generation != ctx.settings.generation()
    {
        return Err(bad(ctx));
    }
    let rr = batch::decode(&rr, IV2Pair::getReservesCall {})?;
    let active = [
        !batch::decode(&coa, IMoto::activeCreatorCall { token: a })?.is_zero(),
        !batch::decode(&cob, IMoto::activeCreatorCall { token: b })?.is_zero(),
    ];
    let ranks = [
        batch::decode(&qa, IMoto::isQuoteCall { token: a })?
            .then_some(batch::decode(&rank0, IMoto::quoteRankCall { token: a })?),
        batch::decode(&qb, IMoto::isQuoteCall { token: b })?
            .then_some(batch::decode(&rank1, IMoto::quoteRankCall { token: b })?),
    ];
    let path_module = if let Ok(module) = ctx.dex_addr("dex.router", f.id) {
        frame::path_module(ctx, f, module, at).await
    } else {
        None
    };
    if generation != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    Ok(MotoswapState {
        path_module,
        sides: Some([(active[0], ranks[0]), (active[1], ranks[1])]),
        fee_router_frozen: frame.frozen,
        reserves: [U256::from(rr.reserve0), U256::from(rr.reserve1)],
        balances: [
            batch::decode(&aa, IMoto::balanceOfCall { owner: pool })?,
            batch::decode(&bb, IMoto::balanceOfCall { owner: pool })?,
        ],
        lp_bps: frame.lp,
        protocol_bps: frame.protocol,
        creator_bps: frame.creator,
        creators: u8::from(active[0]) + u8::from(active[1]),
        fee_on_output: [
            batch::decode(&fa, IMoto::feeOnOutputCall { path: vec![a, b] })?,
            batch::decode(&fbout, IMoto::feeOnOutputCall { path: vec![b, a] })?,
        ],
    })
}
