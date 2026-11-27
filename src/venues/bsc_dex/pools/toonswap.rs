//! Cold read of the factory fee split and pair balances on one canonical Ethereum block.
use super::super::{ctx::Ctx, node::abi::IERC20};
use super::{
    abi_v2v3::IV2Pair, deployment_at_head::Cache, families::Family, load::Loaded,
    nami_batch as batch, solidly_classic_factory,
};
use crate::{
    core::types::{PoolState, ToonState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;
use std::sync::LazyLock;
alloy::sol! {
    function REFERRAL_FEE() external view returns(uint256);
    function TREASURY_FEE() external view returns(uint256);
    function dibs() external view returns(address);
    function treasury() external view returns(address);
}
type Fees = ([U256; 2], [Address; 2]);
static TERMS: LazyLock<Cache<Fees, (B256, Address, B256)>> = LazyLock::new(Cache::default);

async fn terms(ctx: &Ctx, factory: Address, hash: B256, at: &Value) -> GatewayResult<Fees> {
    let head = at["blockHash"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .filter(|_: &B256| at["requireCanonical"] == true)
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "ToonSwap: требуется канонический хеш блока",
            )
        })?;
    let cell = TERMS.at((head, factory, hash));
    cell.get_or_try_init(|| async {
        let [d, t, x, y] = batch::read(
            ctx,
            [
                batch::view(at, factory, REFERRAL_FEECall {}),
                batch::view(at, factory, TREASURY_FEECall {}),
                batch::view(at, factory, dibsCall {}),
                batch::view(at, factory, treasuryCall {}),
            ],
        )
        .await?;
        Ok((
            [
                batch::decode(&d, REFERRAL_FEECall {})?,
                batch::decode(&t, TREASURY_FEECall {})?,
            ],
            [
                batch::decode(&x, dibsCall {})?,
                batch::decode(&y, treasuryCall {})?,
            ],
        ))
    })
    .await
    .copied()
}
pub(crate) async fn read(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tokens: (Address, Address),
    at: &Value,
) -> GatewayResult<Loaded> {
    let rev = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let bal = IERC20::balanceOfCall { owner: pool };
    let rs = IV2Pair::getReservesCall {};
    let calls = [
        batch::view(at, pool, rs.clone()),
        batch::view(at, tokens.0, bal.clone()),
        batch::view(at, tokens.1, bal.clone()),
    ];
    let expected = |key| {
        ctx.dex_text(key, f.id)?.parse::<B256>().map_err(|_| {
            ctx.err(
                ErrorCode::NotConfigured,
                format!("ToonSwap: неверный {key}"),
            )
        })
    };
    let factory_hash = expected("dex.factory_runtime_hash")?;
    let pair_hash = expected("dex.pair_runtime_hash")?;
    let ((), (), (rates, recipients), [r, a, b]) = tokio::try_join!(
        solidly_classic_factory::runtime(ctx, factory, factory_hash, at),
        solidly_classic_factory::runtime(ctx, pool, pair_hash, at),
        terms(ctx, factory, factory_hash, at),
        batch::read(ctx, calls)
    )?;
    if rev != ctx.settings.generation() {
        return Err(ctx.err(ErrorCode::StaleData, "ToonSwap: изменился профиль"));
    }
    let reserves = batch::decode(&r, rs)?;
    let state = ToonState {
        pool,
        reserves: [U256::from(reserves.reserve0), U256::from(reserves.reserve1)],
        balances: [batch::decode(&a, bal.clone())?, batch::decode(&b, bal)?],
        rates,
        recipients,
    };
    Ok(Loaded {
        state: PoolState::ToonSwap(Box::new(state)),
        fee_pips: Some(2500),
        ticks: None,
        bins: None,
    })
}
