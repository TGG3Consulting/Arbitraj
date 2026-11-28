//! Проверка размеров первого запуска по публикациям /user/rate и подлинным полным оплатам Ethereum.
use super::Market;
use crate::{
    settings::recommendations,
    venues::bsc_dex::{
        num,
        tokenlon::{roles::FrozenRoles, wire::abi::FillOrder},
    },
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::Address;
use flate2::read::GzDecoder;
use rust_decimal::Decimal;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn tokenlon_extra_usdt_markets_match_personal_rates_and_onchain_base_sizes() {
    let snapshot: Value = serde_json::from_slice(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_additional_rates.json"
    ))
    .unwrap();
    let history: Value = serde_json::from_reader(GzDecoder::new(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_expired_pmm.json.gz"
        )
        .as_slice(),
    ))
    .unwrap();
    let registry: Value = serde_json::from_slice(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_roles.json"
    ))
    .unwrap();
    let roles: Vec<String> = serde_json::from_value(registry["roles"].clone()).unwrap();
    let pmm = FrozenRoles::from_rows(registry["chain"].as_u64().unwrap(), &roles)
        .unwrap()
        .pmm
        .address;
    let configured = recommendations::venue_value("tokenlon.pairs", "eth_dex").unwrap();
    let markets: BTreeMap<_, _> = configured
        .split(',')
        .map(|s| Market::parse(s).unwrap())
        .map(|m| (m.base.clone(), m))
        .collect();
    let tokens = snapshot["tokens"].as_array().unwrap();
    let token = |name: &str| tokens.iter().find(|t| t["symbol"] == name).unwrap();
    let address = |t: &Value| {
        t["contractAddress"]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap()
    };
    assert_eq!(snapshot["auth"]["method"], "auth.getSdkJwtToken");
    assert_eq!(snapshot["auth"]["http"], 200);
    assert_eq!(snapshot["strategy"], "PMMV5");
    for sym in ["WBTC", "UNI"] {
        let m = &markets[sym];
        let t = token(sym);
        let cash = token("USDT");
        assert_eq!(t["chainId"], 1);
        assert_eq!(m.base_token, address(t));
        assert_eq!(m.quote_token, address(cash));
        assert!(t["opposites"].as_array().unwrap().contains(&cash["symbol"]));
        assert!(cash["opposites"].as_array().unwrap().contains(&t["symbol"]));
        let pair = format!("{sym}_USDT");
        let mut sides = BTreeSet::new();
        let mut sizes = BTreeSet::new();
        for row in snapshot["rates"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["pair"] == pair)
        {
            let side = row["side"].as_str().unwrap();
            let qty = row["amount"].as_str().unwrap();
            let response = &row["response"];
            assert_eq!(row["topic"], format!("/user/rate/{pair}/{side}/{qty}"));
            assert_eq!(response["exchangeable"], true);
            assert!(
                response["rate"]
                    .as_str()
                    .unwrap()
                    .parse::<Decimal>()
                    .unwrap()
                    > Decimal::ZERO
            );
            let n = qty.parse::<Decimal>().unwrap();
            for (field, minimum) in [("minAmount", true), ("maxAmount", false)] {
                let bound = response[field].to_string().parse::<Decimal>().unwrap();
                assert!(if minimum { n >= bound } else { n <= bound });
            }
            sides.insert(side);
            sizes.insert(n);
        }
        assert_eq!(sides, BTreeSet::from(["BUY", "SELL"]));
        assert_eq!(sizes, m.sizes.iter().copied().collect());
        let mut settled = BTreeSet::new();
        for row in history["rows"].as_array().unwrap() {
            for raw in row["logs"].as_array().unwrap() {
                let l: Log = Log {
                    inner: serde_json::from_value(raw.clone()).unwrap(),
                    ..Default::default()
                };
                if l.address() != pmm || l.topics().first() != Some(&FillOrder::SIGNATURE_HASH) {
                    continue;
                }
                let f = FillOrder::decode_log_data_validate(&l.inner.data).unwrap();
                let n = if f.takerAssetAddr == m.base_token && f.makerAssetAddr == m.quote_token {
                    f.takerAssetAmount
                } else if f.makerAssetAddr == m.base_token && f.takerAssetAddr == m.quote_token {
                    f.makerAssetAmount
                } else {
                    continue;
                };
                settled.insert(num::to_decimal(n, t["decimal"].as_u64().unwrap() as u8).unwrap());
            }
        }
        assert_eq!(sizes, settled);
    }
}
