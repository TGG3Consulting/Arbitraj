//! Исходные поручения и независимые денежные события Ethereum, включая фактически выбранный Uniswap.
use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;
fn read(bytes: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
}
fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn make(args: &IProp::swapViaVenueV1Call, owner: Address, fixture: &Value) -> Proof {
    let d = &fixture["canonical"];
    let roles = &fixture["roles"]["roles"];
    let venue = args.venue;
    let vault = if venue == address(&roles["Goliath"]["address"]) {
        address(&roles["goliath_vault"]["address"])
    } else {
        address(&roles["tempest_vault"]["address"])
    };
    let router = address(&d["router"]);
    Proof {
        terms: Terms {
            target: router,
            router,
            venue,
            vault,
            fallback: address(&roles["uniswap"]["address"]),
            fallback_allowed: true,
            code: Code {
                shell: d["runtime"]["hash"].as_str().unwrap().parse().unwrap(),
                implementation: address(&d["implementation"]["address"]),
                runtime: d["implementation"]["hash"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
            },
            venue_code: None,
            vault_code: None,
        },
        payer: owner,
        input: args.tokenIn,
        output: args.tokenOut,
        amount: args.amountIn,
        minimum: args.minOut,
        deadline: args.deadline,
    }
}
#[test]
fn signed_named_payment_and_actual_uniswap_are_recovered_from_original_logs() {
    let named = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/propamm_router_ethereum.json.gz"
    ));
    let separate = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/propamm_ethereum_wallets.json.gz"
    ));
    let mut count = [0usize; 3];
    let mut inspect = |data: &Value,
                       owner: &Value,
                       list: &Value,
                       spent: U256,
                       received: U256,
                       expected: Address| {
        let raw = data.as_str().unwrap().parse::<Bytes>().unwrap();
        let args = IProp::swapViaVenueV1Call::abi_decode_validate(&raw).unwrap();
        let p = make(&args, address(owner), &named);
        let transfers: Vec<SimLog> = serde_json::from_value(list.clone()).unwrap();
        let logs = signed_call::logs(&transfers).unwrap();
        assert_eq!(p.data(), raw);
        assert_eq!(p.outcome(&logs), Some((expected, spent, received)));
        let removed: Vec<_> = logs
            .iter()
            .filter(|l| l.address() != p.output)
            .cloned()
            .collect();
        assert_eq!(p.outcome(&removed), None);
        let mut limit = p.clone();
        limit.minimum = received + U256::ONE;
        assert!(limit.outcome(&logs).is_none());
        if expected == p.terms.fallback {
            count[2] += 1;
        } else {
            let mut foreign = p.clone();
            foreign.terms.vault = Address::with_last_byte(9);
            assert!(foreign.outcome(&logs).is_none());
            if expected == address(&named["roles"]["roles"]["Goliath"]["address"]) {
                count[0] += 1
            } else {
                count[1] += 1
            }
        }
    };
    for row in named["ordinary"].as_array().unwrap() {
        let args = IProp::swapViaVenueV1Call::abi_decode_validate(
            &row["data"].as_str().unwrap().parse::<Bytes>().unwrap(),
        )
        .unwrap();
        inspect(
            &row["data"],
            &row["payer"],
            &row["logs"],
            args.amountIn,
            U256::from_str_radix(row["received"].to_string().trim_matches('"'), 10).unwrap(),
            args.venue,
        );
    }
    for row in separate["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["status"] == "paid" && v["order"]["to"] == named["canonical"]["router"])
    {
        let n = |k: &str| U256::from_str_radix(&row[k].to_string(), 10).unwrap();
        inspect(
            &row["order"]["data"],
            &separate["owner"],
            &row["logs"],
            n("paid"),
            n("net"),
            address(&row["actual"]),
        );
    }
    assert_eq!(count, [13, 4, 16]);
    let mut guarded = 0;
    let mut strict = 0;
    for row in separate["rows"].as_array().unwrap() {
        if row["form"] == "strict" {
            assert_eq!(row["status"], "reverted");
            assert_eq!(row["paid"], 0);
            assert_eq!(row["net"], 0);
            strict += 1;
            continue;
        }
        if row["form"] != "guarded-fallback" {
            continue;
        }
        let raw: Bytes = row["order"]["data"].as_str().unwrap().parse().unwrap();
        let a = IStrict::executeWithFallbackCall::abi_decode_validate(&raw).unwrap();
        let owner = address(&separate["owner"]);
        let mut proof = make(
            &IProp::swapViaVenueV1Call {
                venue: a.venue,
                tokenIn: a.input,
                tokenOut: a.output,
                amountIn: a.amount,
                minOut: a.minimum,
                recipient: owner,
                deadline: a.deadline,
            },
            owner,
            &named,
        );
        proof.terms.target = address(&separate["module"]);
        assert_eq!(a.fallbackRouter, proof.terms.fallback);
        assert_eq!(proof.data(), raw);
        let sim: Vec<SimLog> = serde_json::from_value(row["logs"].clone()).unwrap();
        let received = U256::from_str_radix(&row["net"].to_string(), 10).unwrap();
        assert_eq!(proof.measured(&sim), Some((a.amount, received)));
        proof.terms.fallback_allowed = false;
        assert!(proof.measured(&sim).is_none());
        guarded += 1;
    }
    assert_eq!((strict, guarded), (16, 16));
}
