//! Положительные и отрицательные записи публичного BSC. Каждый полный образец
//! содержит чтения двух держателей и runtime из того же канонического блока.
use super::*;
use serde_json::Value;
use std::io::Read as _;
fn recorded(file: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/presell_bsc_2026-11-27")
        .join(file);
    let mut plain = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap())
        .read_to_end(&mut plain)
        .unwrap();
    serde_json::from_slice(&plain).unwrap()
}
pub(super) fn fixture(name: &str) -> Value {
    recorded(&format!("{name}_with_code.json.gz"))
}
pub(super) fn parse<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}
fn replay(name: &str) -> Result<(), String> {
    replay_capture(name, &fixture(name))
}
fn replay_capture(name: &str, v: &Value) -> Result<(), String> {
    let t: Address = parse(&v["token"]);
    let calls = v["samples"].as_array().unwrap();
    let samples: [observed::Access; 2] =
        [0, 1].map(|i| parse(&calls[i]["access"]["response"]["result"]));
    let (fields, reads) = observed::compare(&samples[0], &samples[1], t)?;
    let code = Code {
        account: t,
        bytecode: parse(&v["code"]["response"]["result"]),
    };
    let extra = reads.other.map(|target| {
        let rpc = &v["dependencyCode"][0];
        assert_eq!(parse::<Address>(&rpc["request"]["params"][0]), target);
        Code {
            account: target,
            bytecode: parse(&rpc["response"]["result"]),
        }
    });
    let admin = if reads.admin {
        Some(parse(&v["proxyStorage"][1]["response"]["result"]))
    } else {
        None
    };
    if reads.implementation {
        let ptr: B256 = parse(&v["proxyStorage"][0]["response"]["result"]);
        assert_eq!(Some(Address::from_word(ptr)), reads.other);
    }
    for (i, sample) in calls.iter().enumerate() {
        let holder = parse(&sample["holder"]);
        let call = &sample["access"]["request"]["params"][0];
        let field = fields[i];
        let block = &sample["access"]["request"]["params"][1];
        if block.is_object() {
            assert_eq!(block["blockHash"], v["blockHash"]);
            assert_eq!(block["requireCanonical"], true);
        } else {
            assert_eq!(block, &v["block"]);
        }
        assert_eq!(
            parse::<Bytes>(&call["data"]),
            probe_calls::balance(parse(&call["from"]), t, holder).input
        );
        let max = (Read {
            token: &code,
            implementation: extra.as_ref(),
            admin,
            caller: parse(&call["from"]),
            holder,
            key: field.key,
            storage: field.storage,
        })
        .mask()
        .ok_or_else(|| format!("{name}: исходный getter кошелька {holder}"))?;
        let field = field.bounded(max);
        if name == "xvs" {
            assert_eq!(max, (U256::from(1) << 96) - U256::from(1));
            assert!(field.credit(max + U256::from(1)).is_none());
        }
        if i == 0
            && let Some(prior) = v["prior"].as_array()
        {
            for case in prior {
                let amount =
                    U256::from_str_radix(case["amount_units"].as_str().unwrap(), 10).unwrap();
                if field.credit(amount).is_none() {
                    return Err(format!(
                        "{name}: объём {amount} превышает максимум поля {max}"
                    ));
                }
            }
        }
        if let Some(raw) = sample.get("storage") {
            assert_eq!(
                parse::<Address>(&raw["request"]["params"][0]),
                field.storage
            );
            assert_eq!(parse::<B256>(&raw["request"]["params"][1]), field.key);
            let value = parse::<Bytes>(&sample["balance"]["response"]["result"]);
            balance_word(&value, parse(&raw["response"]["result"]))?;
        }
        let credit = field.credit(max).unwrap();
        assert_eq!(credit.address, field.storage);
        assert_eq!(
            credit.state,
            vec![(field.key, B256::from(max.to_be_bytes::<32>()))]
        );
    }
    Ok(())
}
#[test]
fn recorded_wbnb_and_previously_working_nxt_use_observed_full_word() {
    replay("wbnb").unwrap();
    replay("nxt").unwrap();
}
#[test]
fn recorded_era_and_usdt_use_observed_full_word() {
    replay("era").unwrap();
    replay("usdt").unwrap();
}
#[test]
fn reflection_read_sets_never_reach_credit() {
    for name in ["pig", "smars"] {
        assert!(replay(name).is_err());
    }
}
#[test]
fn packed_word_is_rejected() {
    let bal = B256::from(U256::from(12).to_be_bytes::<32>());
    let mut packed = bal;
    packed.0[0] = 1;
    assert!(balance_word(bal.as_slice(), packed).is_err());
    assert!(balance_word(&bal.as_slice()[1..], bal).is_err());
    assert_eq!(balance_word(bal.as_slice(), bal), Ok(U256::from(12)));
}

#[test]
fn recorded_xvs_masks_its_uint96_balance() {
    let v = fixture("xvs");
    assert_eq!(v["balanceLayout"]["type"]["value"], "t_uint96");
    replay("xvs").unwrap();
}

/// У этих кошельков журнал подтвердил полную проверку до покупки на старой схеме.
/// Сопоставляем доступность поля на новом записанном состоянии, отдельно от PnL.
#[test]
fn getter_coverage_for_previously_verified_bsc_sales() {
    let capture = recorded("previous_credit_corpus.json.gz");
    let rows = capture.as_array().unwrap();
    let mut misses = Vec::new();
    let mut evidence = std::collections::BTreeSet::new();
    let mut reasons = 0;
    let mut observations = 0;
    for row in rows {
        let ticker = row["symbol"].as_str().unwrap();
        let cases = row["prior"].as_array().unwrap();
        assert!(!cases.is_empty());
        assert_eq!(cases[0]["holder"], row["samples"][0]["holder"]);
        for case in cases {
            assert!(
                case["funding"]
                    .as_str()
                    .unwrap()
                    .contains("слот проверен balanceOf")
            );
            reasons += 1;
            observations += case["observations"].as_u64().unwrap();
            assert_eq!(case["holder"], row["samples"][0]["holder"]);
            evidence.insert((
                case["source"].as_str().unwrap(),
                case["id"].as_str().unwrap(),
            ));
        }
        if let Err(why) = replay_capture(ticker, row) {
            misses.push(format!("{ticker} ({}): {why}", cases[0]["id"]));
        }
    }
    assert_eq!(
        (rows.len(), evidence.len(), reasons, observations),
        (12, 18, 27, 185)
    );
    assert!(misses.is_empty(), "{}", misses.join(", "));
}
