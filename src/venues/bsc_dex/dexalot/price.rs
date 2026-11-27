//! The requested side trades base against quote. isbase describes the fixed amount and is independent of side.
use super::{Query, client, fail};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, num},
};
use alloy_primitives::{Address, U256};
use serde::Deserialize;
use std::collections::BTreeMap;
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Pair {
    base: String,
    quote: String,
    pub base_address: Address,
    pub quote_address: Address,
    base_decimals: u8,
    quote_decimals: u8,
}
impl Pair {
    pub(super) fn name(&self) -> String {
        format!("{}/{}", self.base, self.quote)
    }
    pub(super) fn precision(&self, a: Address) -> Option<u8> {
        if a == self.base_address {
            Some(self.base_decimals)
        } else if a == self.quote_address {
            Some(self.quote_decimals)
        } else {
            None
        }
    }
}
pub(super) struct Pairs(BTreeMap<String, Pair>);
impl Pairs {
    pub(super) async fn load(ctx: &Ctx, chain: u64) -> GatewayResult<Self> {
        let all: BTreeMap<String, Pair> =
            client::request(ctx, "pairs", &[("chainid", chain.to_string())], None).await?;
        Ok(Self(
            all.into_iter()
                .filter(|(name, p)| p.base_address != p.quote_address && p.name() == *name)
                .collect(),
        ))
    }
    pub(super) fn find(&self, a: Address, b: Address) -> Option<&Pair> {
        let mut found = self
            .0
            .values()
            .filter(|p| p.precision(a).is_some() && p.precision(b).is_some());
        let p = found.next()?;
        found.next().is_none().then_some(p)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    success: bool,
    pair: String,
    side: u8,
    base_amount: String,
    quote_amount: String,
}
pub(super) struct Level<'a> {
    pub spent: U256,
    pub net: U256,
    pub physical: &'a Pair,
}
fn human(raw: U256, dec: u8) -> String {
    let n = raw.to_string();
    let d = usize::from(dec);
    if d == 0 {
        return n;
    }
    if n.len() <= d {
        format!("0.{}{}", "0".repeat(d - n.len()), n)
    } else {
        format!("{}.{}", &n[..n.len() - d], &n[n.len() - d..])
    }
}
fn rounded(value: &str, dec: u8, ceil: bool) -> Option<U256> {
    if let Some(v) = num::exact_amount(None, Some(value), dec) {
        return Some(v);
    }
    let (whole, frac) = value.split_once('.')?;
    if whole.is_empty()
        || !whole.bytes().all(|x| x.is_ascii_digit())
        || !frac.bytes().all(|x| x.is_ascii_digit())
        || frac.len() <= usize::from(dec)
    {
        return None;
    }
    let (keep, extra) = frac.split_at(usize::from(dec));
    let floor = U256::from_str_radix(&format!("{whole}{keep}"), 10).ok()?;
    let v = floor.checked_add(U256::from(u8::from(
        ceil && extra.bytes().any(|x| x != b'0'),
    )))?;
    (!v.is_zero()).then_some(v)
}
fn consistent(a: &Pair, b: &Pair, w: Address) -> bool {
    let aliases = |x, y| {
        x == y
            || (!w.is_zero() && ((x == Address::ZERO && y == w) || (y == Address::ZERO && x == w)))
    };
    aliases(a.base_address, b.base_address)
        && aliases(a.quote_address, b.quote_address)
        && a.base_decimals == b.base_decimals
        && a.quote_decimals == b.quote_decimals
}
pub(super) async fn level<'a>(
    ctx: &Ctx,
    pairs: &'a Pairs,
    pair: &'a Pair,
    q: Query,
    chain: u64,
) -> GatewayResult<Level<'a>> {
    let bad = || {
        fail(
            ctx,
            "Dexalot: сторона, размер или физическая пара ответа не совпали",
        )
    };
    let sell = q.input == pair.base_address;
    let fixed = if q.by_target { q.output } else { q.input };
    let resp: Response = client::request(
        ctx,
        "pairprice",
        &[
            ("chainid", chain.to_string()),
            ("pair", pair.name()),
            (
                "amount",
                human(q.qty, pair.precision(fixed).ok_or_else(bad)?),
            ),
            ("side", u8::from(sell).to_string()),
            ("isbase", u8::from(fixed == pair.base_address).to_string()),
        ],
        None,
    )
    .await?;
    decode(pairs, pair, q, resp, ctx.addr("dex.native_wrapped").ok()).ok_or_else(bad)
}
fn decode<'a>(
    pairs: &'a Pairs,
    pair: &Pair,
    q: Query,
    resp: Response,
    wrapped: Option<Address>,
) -> Option<Level<'a>> {
    let sell = q.input == pair.base_address;
    let fixed = if q.by_target { q.output } else { q.input };
    let physical = pairs.0.get(&resp.pair)?;
    if !resp.success
        || resp.side != u8::from(sell)
        || (pair != physical && !consistent(pair, physical, wrapped?))
    {
        return None;
    }
    let (source, out) = if sell {
        (&resp.base_amount, &resp.quote_amount)
    } else {
        (&resp.quote_amount, &resp.base_amount)
    };
    let raw_fixed = if q.by_target { out } else { source };
    if num::exact_amount(None, Some(raw_fixed), pair.precision(fixed)?) != Some(q.qty) {
        return None;
    }
    let spent = rounded(source, pair.precision(q.input)?, true)?;
    let net = rounded(out, pair.precision(q.output)?, false)?;
    Some(Level {
        spent,
        net,
        physical,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn published_pairprice_observations_preserve_the_fixed_side_and_name_the_native_alias() {
        use serde_json::Value;
        let j: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/dexalot_base_indicative.json"
        ))
        .unwrap();
        let pairs = Pairs(serde_json::from_value(j["pairs"].clone()).unwrap());
        let weth = pairs.0["WETH/USDC"].base_address;
        let mut outcomes = [0usize; 3];
        for row in j["prices"].as_array().unwrap() {
            let params = &row["params"];
            let data = row.get("retry").unwrap_or(&row["answer"]);
            if data["success"] != true {
                outcomes[2] += 1;
                continue;
            }
            let pair = &pairs.0[params["pair"].as_str().unwrap()];
            let sell = params["side"] == "1";
            let fixed_base = params["isbase"] == "1";
            let (input, output) = if sell {
                (pair.base_address, pair.quote_address)
            } else {
                (pair.quote_address, pair.base_address)
            };
            let dec = if fixed_base {
                pair.base_decimals
            } else {
                pair.quote_decimals
            };
            let qty = num::exact_amount(None, params["amount"].as_str(), dec).unwrap();
            let q = Query {
                input,
                output,
                qty,
                by_target: sell != fixed_base,
            };
            let ans = decode(
                &pairs,
                pair,
                q,
                serde_json::from_value(data.clone()).unwrap(),
                Some(weth),
            )
            .unwrap();
            let alias = ans.physical != pair;
            outcomes[usize::from(alias)] += 1;
            assert_eq!(if q.by_target { ans.net } else { ans.spent }, qty);
            if alias {
                assert_eq!(
                    (pair.base_address, ans.physical.base_address),
                    (weth, Address::ZERO)
                );
            }
        }
        assert!(outcomes.iter().all(|&n| n > 0));
    }
    #[test]
    fn atoms_keep_exact_fixed_leg_and_round_variable_cash_in_the_owners_direction() {
        assert_eq!(human(U256::from(1_000_001u64), 6), "1.000001");
        assert_eq!(rounded("1.66623298", 6, true), Some(U256::from(1_666_233)));
        assert_eq!(rounded("1.66623298", 6, false), Some(U256::from(1_666_232)));
        assert_eq!(rounded("0.00000001", 6, true), Some(U256::ONE));
        assert_eq!(rounded("-0.123", 2, true), None);
    }
}
