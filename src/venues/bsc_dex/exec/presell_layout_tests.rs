//! Положительные и отрицательные записи публичного BSC. Каждый полный образец
//! содержит чтения двух держателей и runtime из того же канонического блока.
use super::*;
use serde_json::Value;
use std::io::Read as _;
fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/presell_bsc_2026-11-27")
        .join(format!("{name}_with_code.json.gz"));
    let mut plain = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap())
        .read_to_end(&mut plain)
        .unwrap();
    serde_json::from_slice(&plain).unwrap()
}
fn parse<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}
fn replay(name: &str) -> Result<(), String> {
    let v = fixture(name);
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
        assert_eq!(
            sample["access"]["request"]["params"][1]["blockHash"],
            v["blockHash"]
        );
        assert_eq!(
            parse::<Bytes>(&call["data"]),
            probe_calls::balance(parse(&call["from"]), t, holder).input
        );
        if !(Read {
            token: &code,
            implementation: extra.as_ref(),
            admin,
            caller: parse(&call["from"]),
            holder,
            key: field.key,
            storage: field.storage,
        })
        .proves()
        {
            return Err(format!(
                "{name}: исходный getter не доказал целое слово кошелька {holder}"
            ));
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
        let credit = field.credit(U256::MAX);
        assert_eq!(credit.address, field.storage);
        assert_eq!(credit.state, vec![(field.key, B256::repeat_byte(255))]);
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
    assert!(replay("xvs").unwrap_err().contains("исходный getter"));
}
