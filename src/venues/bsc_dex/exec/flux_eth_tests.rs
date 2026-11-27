//! По каждому принятому нативному обмену восстанавливает calldata, сумму ETH и обе стороны квитанции.
use super::flux_eth::{Market, Proof};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::Value;
use std::str::FromStr;
fn parse<T: FromStr>(v: &Value) -> T
where
    T::Err: std::fmt::Debug,
{
    v.as_str().unwrap().parse().unwrap()
}
#[test]
fn signed_native_order_and_recovered_wallet_amounts_match_every_recorded_positive_leg() {
    let books: [(&[u8], bool); 2] = [
        (
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/base_native_cl_wallets.json.gz"
            ),
            false,
        ),
        (
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/base_native_bin_wallets.json.gz"
            ),
            true,
        ),
    ];
    for (bytes, bin) in books {
        let book: Value = serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap();
        let wrapped: Address = parse(&book["module"]["roles"][1]);
        for (collection, module) in [(&book["native_router"], false), (&book, true)] {
            let mut count = 0;
            for m in collection["markets"].as_array().unwrap() {
                for c in m["cases"].as_array().unwrap() {
                    if c["mode"]
                        != if module {
                            "native_module"
                        } else {
                            "router_exact"
                        }
                    {
                        continue;
                    }
                    for leg in [&c["buy"], &c["sell"]]
                        .into_iter()
                        .filter(|l| l["stage"] == "accepted")
                    {
                        let a = leg["calls"].as_array().unwrap();
                        let a = &a[a.len() - 3];
                        let r = leg["results"].as_array().unwrap();
                        let r = &r[r.len() - 3];
                        let data: Bytes = parse(&a["data"]);
                        let pos = 4 + 32 * if module { 5 } else { 2 };
                        let value = a.get("value").map(parse::<U256>).unwrap_or_default();
                        let token: Address = parse(&m["key"][1]);
                        let (input, output) = if value.is_zero() {
                            (token, wrapped)
                        } else {
                            (wrapped, token)
                        };
                        let market = Market {
                            router: parse(&a["to"]),
                            permit: parse(&collection["roles"]["dex.permit2"]),
                            wrapped,
                            token,
                            manager: parse(&m["key"][3]),
                            id: parse(&m["pool"]),
                            bin,
                            fee: m["key"][4].as_u64().unwrap() as u32,
                            parameters: parse(&m["key"][5]),
                            module,
                        };
                        let mut p = Proof {
                            market,
                            payer: parse(&a["from"]),
                            input,
                            output,
                            amount: parse(&leg["paid"]),
                            minimum: parse(&leg["net"]),
                            deadline: U256::from_be_slice(&data[pos..pos + 32]),
                        };
                        assert_eq!(p.data(), data);
                        assert_eq!(p.value(), value);
                        let logs = serde_json::from_value::<Vec<_>>(r["logs"].clone()).unwrap();
                        assert_eq!(p.measured(&logs), Some((p.amount, p.minimum)));
                        p.market.id = B256::ZERO;
                        assert_eq!(p.measured(&logs), None);
                        count += 1;
                    }
                }
            }
            if !module
                || collection["markets"].as_array().unwrap().iter().any(|m| {
                    m["cases"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|c| c["mode"] == "native_module")
                })
            {
                assert!(count >= if bin { 6 } else { 28 });
            }
        }
    }
}
