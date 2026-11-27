//! Реестр и оплата на Base по двум независимым размерам у десяти поколений хуков.
use super::{basestonk, launch_keys::Role};
use alloy_primitives::{Address, B256, U256, keccak256};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

fn snapshot(bytes: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
}
fn addr(value: &Value) -> Address {
    value.as_str().unwrap().parse().unwrap()
}
fn n(value: &Value) -> U256 {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
        .parse()
        .unwrap()
}

#[test]
fn watched_pair_and_cold_pool_id_restore_the_same_registered_key() {
    let d = snapshot(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/basestonk_base_directory.json.gz"
    ));
    let profile: Value =
        serde_json::from_slice(include_bytes!("../../../../config/first_run_dex.json")).unwrap();
    let settings = &profile
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "basestonk-base")
        .unwrap()["settings"];
    let roles = settings["dex.launch_roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let mut fields = row.as_str().unwrap().split('@');
            Role {
                kind: fields.next().unwrap().into(),
                address: fields.next().unwrap().parse().unwrap(),
                hash: fields.next().unwrap().parse().unwrap(),
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(settings["dex.kind"], "launch_basestonk");
    assert_eq!(roles.iter().filter(|r| r.kind == "hook").count(), 11);
    let mut count = 0;
    let mut liquid = 0;
    for r in d["markets"].as_array().unwrap() {
        let (token, other) = (addr(&r["token"]), addr(&r["pair"]));
        let id: B256 = r["id"].as_str().unwrap().parse().unwrap();
        let expected = basestonk::key(
            (token, other),
            addr(&r["hook"]),
            r["key"][2].as_u64().unwrap() as u32,
        );
        assert_eq!(expected.id(), id);
        assert_eq!(
            basestonk::restore(&roles, (other, token), id)
                .unwrap()
                .abi(),
            expected.abi()
        );
        let words = r["config"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|x| U256::from(x.as_u64().unwrap()).to_be_bytes::<32>())
            .collect::<Vec<_>>();
        let terms = basestonk::config(&words).unwrap();
        assert_eq!(terms.token_is_zero, token < other);
        count += 1;
        liquid += usize::from(r["liquidity"].as_str().unwrap().parse::<u128>().unwrap() > 0);
    }
    assert_eq!((count, liquid), (4781, 1580));
    assert!(basestonk::config(&[0u8; 128]).is_none());
    assert!(basestonk::restore(&roles, (Address::ZERO, Address::ZERO), B256::ZERO).is_none());
}

#[test]
fn one_payment_even_when_the_hook_sells_its_tax_in_the_same_pool() {
    let transfer = format!("{:#x}", keccak256("Transfer(address,address,uint256)"));
    let taken = format!(
        "{:#x}",
        keccak256("FeeTaken(bytes32,address,uint256,uint256)")
    );
    let swap = format!(
        "{:#x}",
        keccak256("Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)")
    );
    let mut compared = HashMap::new();
    let mut hooks = HashSet::new();
    let mut doubles = 0;
    for bytes in [
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/basestonk_base_wallets.json.gz"
        )[..],
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/basestonk_base_secondary.json.gz"
        )[..],
    ] {
        let d = snapshot(bytes);
        let payer = format!("{:#x}", addr(&d["payer"]).into_word());
        for row in d["results"].as_array().unwrap() {
            assert_eq!(row["success"], true);
            assert_eq!(row["strict"]["success"], false);
            if row["mode"] == "v5" {
                assert_eq!(row["expired"]["code"], 3);
            }
            let mut out = U256::ZERO;
            let mut incoming = U256::ZERO;
            let mut refund = U256::ZERO;
            let mut sent_back = U256::ZERO;
            let mut fees = 0;
            let mut swaps = 0;
            for log in row["logs"].as_array().unwrap() {
                let topics = &log["topics"];
                if topics[0] == taken && topics[1] == row["id"] && log["address"] == row["hook"] {
                    fees += 1;
                }
                if topics[0] == swap && topics[1] == row["id"] {
                    swaps += 1;
                }
                if topics[0] != transfer {
                    continue;
                }
                let qty = n(&log["data"]);
                if log["address"] == row["src"] {
                    if topics[1] == payer {
                        out += qty;
                    }
                    if topics[2] == payer {
                        refund += qty;
                    }
                }
                if log["address"] == row["dst"] {
                    if topics[2] == payer {
                        incoming += qty;
                    }
                    if topics[1] == payer {
                        sent_back += qty;
                    }
                }
            }
            assert_eq!(out - refund, n(&row["paid"]));
            assert_eq!(n(&row["paid"]), n(&row["requested"]));
            assert_eq!(incoming - sent_back, n(&row["received"]));
            assert_eq!(fees, 1);
            assert_eq!(swaps, row["pool_swaps"].as_u64().unwrap());
            doubles += usize::from(swaps == 2);
            let k = (
                row["id"].to_string(),
                row["src"].to_string(),
                n(&row["requested"]),
            );
            compared
                .entry(k)
                .or_insert_with(Vec::new)
                .push((row["mode"].to_string(), n(&row["received"])));
            hooks.insert(row["hook"].to_string());
        }
    }
    assert_eq!((compared.len(), hooks.len(), doubles), (40, 10, 20));
    for modes in compared.values() {
        assert_eq!(modes.len(), 2);
        assert_ne!(modes[0].0, modes[1].0);
        assert_eq!(modes[0].1, modes[1].1);
    }
}
