//! Share the fee-acquisition markets across all ordinary pairs at the same canonical block.
use super::super::{check::sim_base::Base, ctx::Ctx, quote::white_state::Head};
use super::antfarm_oracle::Observation;
use super::{
    antfarm::{self, ATF, FACTORY, IFarm},
    families::Family,
    nami_batch as batch,
};
use crate::{
    core::types::{AntfarmBook, AntfarmMarket},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, LazyLock, Mutex},
};
use tokio::sync::OnceCell;

pub(in crate::venues::bsc_dex) use super::antfarm::ATF as FEE_TOKEN;

type Source = (AntfarmMarket, Option<U256>);
type Cell = Arc<OnceCell<Vec<Source>>>;
type Key = (B256, Address);
static CACHE: LazyLock<Mutex<VecDeque<(Key, Cell)>>> = LazyLock::new(Mutex::default);

fn cell(key: Key) -> Cell {
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, v)) = c.iter().find(|(k, _)| *k == key) {
        return v.clone();
    }
    if c.len() >= 96 {
        c.pop_front();
    }
    let v = Arc::new(OnceCell::new());
    c.push_back((key, v.clone()));
    v
}

async fn projected(
    ctx: &Ctx,
    pool: Address,
    market: &AntfarmMarket,
    at: &Value,
    now: u32,
) -> GatewayResult<Option<U256>> {
    let [o, p, res] = batch::read(
        ctx,
        [
            batch::view(at, pool, IFarm::antfarmOracleCall {}),
            batch::view(at, pool, IFarm::price1CumulativeLastCall {}),
            batch::view(at, pool, IFarm::getReservesCall {}),
        ],
    )
    .await?;
    let oracle = batch::decode(&o, IFarm::antfarmOracleCall {})?;
    let cum = batch::decode(&p, IFarm::price1CumulativeLastCall {})?;
    let pair_last = batch::decode(&res, IFarm::getReservesCall {})?.last;
    let query = batch::read(
        ctx,
        [
            batch::view(at, oracle, IFarm::pairCall {}),
            batch::view(at, oracle, IFarm::price1CumulativeLastCall {}),
            batch::view(at, oracle, IFarm::blockTimestampLastCall {}),
            batch::view(at, oracle, IFarm::firstUpdateCallCall {}),
            batch::view(at, oracle, IFarm::price1AverageCall {}),
        ],
    );
    let [back, old, ts, first, avg] = query.await?;
    if batch::decode(&back, IFarm::pairCall {})? != pool {
        return Err(ctx.err(ErrorCode::NotApplicable, "Antfarm: другая пара оракула"));
    }
    Ok(Observation {
        pair_cumulative: cum,
        oracle_cumulative: batch::decode(&old, IFarm::price1CumulativeLastCall {})?,
        pair_time: pair_last,
        oracle_time: batch::decode(&ts, IFarm::blockTimestampLastCall {})?,
        average: U256::from(batch::decode(&avg, IFarm::price1AverageCall {})?),
        first: batch::decode(&first, IFarm::firstUpdateCallCall {})?,
    }
    .rate(market.reserves, now))
}

async fn sources(
    ctx: &Ctx,
    f: &Family,
    asset: Address,
    at: &Value,
    now: u32,
) -> GatewayResult<Vec<Source>> {
    let request = IFarm::getFeesForPairCall {
        token0: ATF,
        token1: asset,
    };
    let [raw] = batch::read(ctx, [batch::view(at, FACTORY, request.clone())]).await?;
    let mut result = Vec::new();
    for tier in batch::decode(&raw, request)?
        .into_iter()
        .filter(|&t| t != 0)
    {
        let pool = antfarm::derived([ATF, asset], tier);
        let market = antfarm::market(ctx, f, pool, (ATF, asset), at).await?;
        if market.lp_supply <= U256::from(1000) || market.reserves.contains(&U256::ZERO) {
            continue;
        }
        let rate = if tier == 10 {
            projected(ctx, pool, &market, at, now).await?
        } else {
            None
        };
        if tier != 10 || rate.is_some() {
            result.push((market, rate));
        }
    }
    Ok(result)
}

pub(in crate::venues::bsc_dex) async fn read(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    edge: (Address, Address),
    head: Head,
) -> GatewayResult<AntfarmBook> {
    let at = head.selector();
    let market = antfarm::market(ctx, f, pool, edge, &at).await?;
    let mut funding = Vec::new();
    if !market.tokens.contains(&ATF) {
        let now = Base::next_time(head.timestamp.to()) as u32;
        for asset in market.tokens {
            let v = cell((head.hash, asset));
            funding.extend(
                v.get_or_try_init(|| sources(ctx, f, asset, &at, now))
                    .await?
                    .iter()
                    .cloned(),
            );
        }
    }
    Ok(AntfarmBook {
        market,
        funding,
        proceeds: None,
        limits: None,
    })
}

pub(in crate::venues::bsc_dex) async fn module(
    ctx: &Ctx,
    f: &Family,
    at: &Value,
) -> GatewayResult<Address> {
    let m = ctx.dex_addr("dex.router", f.id)?;
    let verified =
        alloy_primitives::b256!("c0e793045e689ae29e80413c4f63de9ddcec8f8691c47a37c90c8709b287e33f");
    if !super::direct_module::configured(ctx, f.id)?.contains(&verified) {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            "Antfarm: отсутствует подтверждённый runtime модуля кошелька",
        ));
    }
    super::solidly_classic_factory::bind(ctx, m, verified, None, at).await?;
    Ok(m)
}
