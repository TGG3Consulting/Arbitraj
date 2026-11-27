//! Share the fee-acquisition markets across all ordinary pairs at the same canonical block.
use super::super::{ctx::Ctx, node::rpc};
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
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, LazyLock, Mutex},
};
use tokio::sync::OnceCell;

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
    hash: B256,
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
            batch::view(at, oracle, IFarm::firstUpdateCall {}),
            batch::view(at, oracle, IFarm::price1AverageCall {}),
        ],
    );
    let header = rpc::call::<Value>(ctx, "eth_getBlockByHash", json!([hash, false]));
    let ([back, old, ts, first, avg], head) = tokio::try_join!(query, header)?;
    let now = head["timestamp"]
        .as_str()
        .and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok())
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "Antfarm: время блока недоступно"))?
        as u32;
    if batch::decode(&back, IFarm::pairCall {})? != pool {
        return Err(ctx.err(ErrorCode::NotApplicable, "Antfarm: другая пара оракула"));
    }
    let elapsed = now.wrapping_sub(batch::decode(&ts, IFarm::blockTimestampLastCall {})?);
    if elapsed < 3600 && !batch::decode(&first, IFarm::firstUpdateCall {})? {
        return Ok(Some(U256::from(batch::decode(
            &avg,
            IFarm::price1AverageCall {},
        )?)));
    }
    if elapsed == 0 || market.reserves.contains(&U256::ZERO) {
        return Ok(None);
    }
    let unit: U256 = (market.reserves[0] << 112) / market.reserves[1];
    let next = cum.checked_add(unit * U256::from(now.wrapping_sub(pair_last)));
    let old = batch::decode(&old, IFarm::price1CumulativeLastCall {})?;
    Ok(
        next.map(|n| {
            (n.wrapping_sub(old) / U256::from(elapsed)) & ((U256::ONE << 224) - U256::ONE)
        }),
    )
}

async fn sources(
    ctx: &Ctx,
    f: &Family,
    asset: Address,
    at: &Value,
    hash: B256,
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
            projected(ctx, pool, &market, at, hash).await?
        } else {
            None
        };
        if tier != 10 || rate.is_some() {
            result.push((market, rate));
        }
    }
    Ok(result)
}

pub(super) async fn read(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    edge: (Address, Address),
    at: &Value,
) -> GatewayResult<AntfarmBook> {
    let market = antfarm::market(ctx, f, pool, edge, at).await?;
    let mut funding = Vec::new();
    if !market.tokens.contains(&ATF) {
        let hash = at["blockHash"]
            .as_str()
            .and_then(|s| s.parse::<B256>().ok())
            .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "Antfarm: нет хеша блока"))?;
        for asset in market.tokens {
            let v = cell((hash, asset));
            funding.extend(
                v.get_or_try_init(|| sources(ctx, f, asset, at, hash))
                    .await?
                    .iter()
                    .cloned(),
            );
        }
    }
    Ok(AntfarmBook { market, funding })
}
