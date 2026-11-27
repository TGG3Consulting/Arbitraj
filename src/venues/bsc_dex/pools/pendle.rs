//! Реестр Pendle. Связывает PT, SY, YT и действующую фабрику на одном каноническом блоке.
use super::{
    families::{self, Family},
    family_variant::Variant,
    nami_batch as batch, stable_list,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, quote::white_state::Head},
};
use alloy::sol;
use alloy_primitives::{Address, B256, U256};
use std::collections::HashMap;

sol! {interface IMarket {
    function factory() external view returns(address);
    function expiry() external view returns(uint256);
    function isValidMarket(address m) external view returns(bool);
    function readTokens() external view returns(address sy,address pt,address yt);
    function SY() external view returns(address);
    function PT() external view returns(address);
    function YT() external view returns(address);
    function isValidTokenIn(address token) external view returns(bool);
    function isValidTokenOut(address token) external view returns(bool);
    function router() external view returns(address);
    function originalFactory() external view returns(address);
    function factoryV3() external view returns(address);
    function factoryV4() external view returns(address);
    function factoryV5() external view returns(address);
    function factoryV6() external view returns(address);
}}
#[derive(Clone, Copy)]
pub(crate) struct Series {
    pub pool: Address,
    pub factory: Address,
    pub sy: Address,
    pub pt: Address,
    pub yt: Address,
}
impl Series {
    fn parse(line: &str) -> Option<(Self, Vec<Address>)> {
        let words: Vec<Address> = line
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        let &[pool, factory, sy, pt, yt, ref rest @ ..] = words.as_slice() else {
            return None;
        };
        let roles = [pool, factory, sy, pt, yt];
        if roles.contains(&Address::ZERO)
            || roles
                .iter()
                .enumerate()
                .any(|(i, a)| roles[..i].contains(a))
        {
            return None;
        }
        Some((
            Self {
                pool,
                factory,
                sy,
                pt,
                yt,
            },
            rest.to_vec(),
        ))
    }
    pub(crate) fn terminal(self, pair: (Address, Address)) -> Option<Address> {
        let t = if pair.0 == self.pt {
            pair.1
        } else if pair.1 == self.pt {
            pair.0
        } else {
            return None;
        };
        (!t.is_zero() && ![self.pt, self.yt, self.pool, self.factory].contains(&t)).then_some(t)
    }
}
pub(crate) fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Pendle: рынок, серия, роли или доступность валюты изменились",
    )
}
struct MarketIndex {
    store: crate::settings::store::Store,
    generation: u64,
    id: String,
    markets: MarketRows,
}
type MarketRows = std::sync::Arc<HashMap<Address, (Series, Vec<Address>)>>;
fn index(ctx: &Ctx, id: &str) -> GatewayResult<MarketRows> {
    static CACHE: std::sync::Mutex<Vec<MarketIndex>> = std::sync::Mutex::new(Vec::new());
    let generation = ctx.settings.generation();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = cache
        .iter()
        .find(|v| v.generation == generation && v.id == id && v.store.same(&ctx.settings))
    {
        return Ok(v.markets.clone());
    }
    let mut map = HashMap::new();
    for row in ctx.dex_list("dex.pendle_markets", id)? {
        let (s, terminals) = Series::parse(&row).ok_or_else(|| bad(ctx))?;
        if map.insert(s.pool, (s, terminals)).is_some() {
            return Err(bad(ctx));
        }
    }
    if ctx.settings.generation() != generation {
        return Err(bad(ctx));
    }
    cache.retain(|x| x.id != id || !x.store.same(&ctx.settings));
    if cache.len() >= 16 {
        cache.remove(0);
    }
    let markets = std::sync::Arc::new(map);
    cache.push(MarketIndex {
        store: ctx.settings.clone(),
        generation,
        id: id.into(),
        markets: markets.clone(),
    });
    Ok(markets)
}
pub(crate) fn series(ctx: &Ctx, id: &str, pool: Address) -> GatewayResult<Series> {
    index(ctx, id)?
        .get(&pool)
        .map(|(s, _)| *s)
        .ok_or_else(|| bad(ctx))
}
#[derive(Clone, Copy)]
pub(super) struct Role {
    pub address: Address,
    pub hash: B256,
    pub implementation: Option<(Address, B256)>,
}
impl Role {
    fn parse(words: &[&str]) -> Option<Self> {
        let [a, h, ref ip @ ..] = *words else {
            return None;
        };
        let address: Address = a.parse().ok()?;
        let hash: B256 = h.parse().ok()?;
        let implementation = match ip {
            [] => None,
            [a, h] => Some((a.parse().ok()?, h.parse().ok()?)),
            _ => return None,
        };
        if address.is_zero()
            || hash.is_zero()
            || implementation
                .is_some_and(|(a, h): (Address, B256)| a.is_zero() || h.is_zero() || a == address)
        {
            return None;
        }
        Some(Self {
            address,
            hash,
            implementation,
        })
    }
    pub(super) async fn check(self, ctx: &Ctx, at: &serde_json::Value) -> GatewayResult<()> {
        super::solidly_classic_factory::bind(ctx, self.address, self.hash, self.implementation, at)
            .await
    }
}
pub(super) fn roles(ctx: &Ctx, id: &str) -> GatewayResult<HashMap<String, Role>> {
    let mut out = HashMap::new();
    for row in ctx.dex_list("dex.pendle_roles", id)? {
        let words: Vec<_> = row.split_whitespace().collect();
        let (key, fields) = words.split_first().ok_or_else(|| bad(ctx))?;
        if out
            .insert(
                (*key).to_string(),
                Role::parse(fields).ok_or_else(|| bad(ctx))?,
            )
            .is_some()
        {
            return Err(bad(ctx));
        }
    }
    Ok(out)
}
pub(crate) async fn available(
    ctx: &Ctx,
    f: &Family,
    s: Series,
    terminal: Address,
    h: Head,
) -> GatewayResult<(bool, bool)> {
    let at = &h.selector();
    let roles = roles(ctx, f.id)?;
    let registrar = roles
        .iter()
        .find(|(k, r)| k.starts_with("marketFactory") && r.address == s.factory)
        .map(|(_, r)| r)
        .ok_or_else(|| bad(ctx))?;
    let quoter = roles.get("routerStatic").ok_or_else(|| bad(ctx))?;
    if quoter.address != ctx.dex_addr("dex.quoter", f.id)? {
        return Err(bad(ctx));
    }
    let (_, _, r) = tokio::try_join!(
        registrar.check(ctx, at),
        quoter.check(ctx, at),
        batch::read(
            ctx,
            [
                batch::view(at, s.factory, IMarket::isValidMarketCall { m: s.pool }),
                batch::view(at, s.pool, IMarket::factoryCall {}),
                batch::view(at, s.pool, IMarket::readTokensCall {}),
                batch::view(at, s.pool, IMarket::expiryCall {}),
                batch::view(at, s.pt, IMarket::SYCall {}),
                batch::view(at, s.pt, IMarket::YTCall {}),
                batch::view(at, s.pt, IMarket::expiryCall {}),
                batch::view(at, s.yt, IMarket::SYCall {}),
                batch::view(at, s.yt, IMarket::PTCall {}),
                batch::view(at, s.yt, IMarket::expiryCall {}),
            ]
        )
    )?;
    let [
        valid,
        fac,
        series,
        exp,
        ptsy,
        ptyt,
        ptexp,
        ytsy,
        ytpt,
        ytexp,
    ] = r;
    let actual = batch::decode(&series, IMarket::readTokensCall {})?;
    let expiry = batch::decode(&exp, IMarket::expiryCall {})?;
    if !batch::decode(&valid, IMarket::isValidMarketCall { m: s.pool })?
        || batch::decode(&fac, IMarket::factoryCall {})? != s.factory
        || (actual.sy, actual.pt, actual.yt) != (s.sy, s.pt, s.yt)
        || expiry <= U256::from(h.timestamp.to::<u64>())
        || batch::decode(&ptsy, IMarket::SYCall {})? != s.sy
        || batch::decode(&ptyt, IMarket::YTCall {})? != s.yt
        || batch::decode(&ytsy, IMarket::SYCall {})? != s.sy
        || batch::decode(&ytpt, IMarket::PTCall {})? != s.pt
        || batch::decode(&ptexp, IMarket::expiryCall {})? != expiry
        || batch::decode(&ytexp, IMarket::expiryCall {})? != expiry
    {
        return Err(bad(ctx));
    }
    if terminal == s.sy {
        return Ok((true, true));
    }
    let (i, o) = (
        IMarket::isValidTokenInCall { token: terminal },
        IMarket::isValidTokenOutCall { token: terminal },
    );
    let [a, b] = batch::read(
        ctx,
        [
            batch::view(at, s.sy, i.clone()),
            batch::view(at, s.sy, o.clone()),
        ],
    )
    .await?;
    Ok((batch::decode(&a, i)?, batch::decode(&b, o)?))
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let check = async {
        let pool = c.pool.parse().map_err(|_| bad(ctx))?;
        let s = series(ctx, c.family.id, pool)?;
        let term = s.terminal((c.coin, c.quote)).ok_or_else(|| bad(ctx))?;
        let h = crate::venues::bsc_dex::quote::white_state::head(ctx, "latest").await?;
        if available(ctx, c.family, s, term, h).await? == (false, false) {
            return Err(bad(ctx));
        }
        Ok(PoolRef::Multi {
            contract: pool,
            token0: c.coin.min(c.quote),
            token1: c.coin.max(c.quote),
        })
    }
    .await;
    match check {
        Ok(reference) => Verdict::Taken(Box::new(super::verify_woo::found(c, reference))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) fn scan(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
) -> stable_list::Extra {
    let mut out = stable_list::Extra::default();
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.variant == Variant::Pendle)
    {
        let Ok(rows) = index(ctx, f.id) else { continue };
        for (s, terminal) in rows.values() {
            let mut terms = terminal.clone();
            terms.push(s.sy);
            terms.sort_unstable();
            terms.dedup();
            for t in terms
                .into_iter()
                .filter(|t| s.terminal((s.pt, *t)).is_some())
            {
                for e in stable_list::candidates(f.id, s.pool, &[s.pt, t], coins, quotes) {
                    out.found.entry(e.base).or_default().push(e);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::HashSet;
    fn archive(data: &[u8]) -> Value {
        serde_json::from_reader(flate2::read::GzDecoder::new(data)).unwrap()
    }
    #[test]
    fn recommended_series_match_the_registered_contracts_and_terminal_assets() {
        for (id, expected, raw) in [
            ("pendle", 85, include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/pendle_ethereum_registry.json.gz").as_slice()),
            ("pendle-base", 22, include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/pendle_base_registry.json.gz").as_slice()),
        ] {
            check_registry(id, expected, archive(raw));
        }
    }

    fn check_registry(id: &str, expected: usize, registry: Value) {
        let rows: Value =
            serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
        let settings = &rows
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap()["settings"];
        let markets = registry["registrations"].as_array().unwrap();
        let mut pools = HashSet::new();
        for seed in settings["dex.pendle_markets"].as_array().unwrap() {
            let (series, terminal) = Series::parse(seed.as_str().unwrap()).unwrap();
            assert!(pools.insert(series.pool));
            let onchain = markets
                .iter()
                .find(|r| r["market"] == series.pool.to_string().to_ascii_lowercase())
                .unwrap();
            assert!(onchain["valid"].as_bool().unwrap());
            assert_eq!(
                onchain["tokens"],
                serde_json::json!([series.sy, series.pt, series.yt])
            );
            assert_eq!(onchain["factory"], serde_json::json!(series.factory));
            assert_eq!(series.terminal((series.sy, series.pt)), Some(series.sy));
            assert_eq!(series.terminal((series.yt, series.pt)), None);
            assert_eq!(series.terminal((series.sy, series.yt)), None);
            let sy = &registry["standardized_yield"][format!("{:#x}", series.sy)];
            for token in terminal {
                assert!([&sy["in"], &sy["out"]].iter().any(|v| {
                    v.as_array()
                        .is_some_and(|a| a.contains(&serde_json::json!(token)))
                }))
            }
        }
        assert_eq!(pools.len(), expected);
        for row in settings["dex.pendle_roles"].as_array().unwrap() {
            let fields: Vec<_> = row.as_str().unwrap().split_whitespace().collect();
            let r = Role::parse(&fields[1..]).unwrap();
            if fields[0].starts_with("0x") {
                continue;
            } // Привязка селекторов отдельно в тестах денежного архива.
            let canonical = &registry["roles"][fields[0]];
            assert_eq!(canonical["address"], serde_json::json!(r.address));
            assert_eq!(canonical["hash"], serde_json::json!(r.hash));
            assert_eq!(
                r.implementation.is_some(),
                canonical["implementation"].is_object()
            );
        }
    }
}
