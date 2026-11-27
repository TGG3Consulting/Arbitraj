//! Комиссия, оставшийся вход и выплата E3 проверяются на всех четырёх реальных контрактах Base.
use alloy_primitives::{Address, Bytes, U256};
use arb::venues::bsc_dex::{
    math::lb_swap,
    pools::{bins::BinMap, load_lb},
};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::BTreeMap;

fn n(v: &Value) -> u128 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn exact_quotes_across_every_occupied_bin_and_the_cash_limit() {
    let d: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!("data/first_run_dex_2026-11-26/e3_base_bins.json.gz")[..],
    ))
    .unwrap();
    let stamp = d["stamp"].as_u64().unwrap();
    let mut total = 0;
    for p in d["pools"].as_array().unwrap() {
        let x = p["x"].as_str().unwrap().parse::<Address>().unwrap();
        let y = p["y"].as_str().unwrap().parse::<Address>().unwrap();
        let step = p["step"].as_u64().unwrap() as u32;
        let words = |v: Vec<u64>| {
            Bytes::from(
                v.into_iter()
                    .flat_map(|w| U256::from(w).to_be_bytes::<32>())
                    .collect::<Vec<_>>(),
            )
        };
        let fields = |k: &str| {
            p[k].as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap())
                .collect()
        };
        let state = load_lb::decode(
            &[
                words(vec![p["active"].as_u64().unwrap()]),
                words(fields("statics")),
                words(fields("variable")),
            ],
            x < y,
            step,
            stamp,
        )
        .unwrap();
        let reserves = p["bins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| (b[0].as_u64().unwrap() as u32, (n(&b[1]), n(&b[2]))))
            .collect::<BTreeMap<_, _>>();
        let bins = BinMap {
            step,
            reserves: reserves.into(),
            range: (0, 0xff_ffff),
        };
        for q in p["quotes"].as_array().unwrap() {
            let r = lb_swap::swap_out(
                &state,
                &bins,
                stamp,
                q["for_y"].as_bool().unwrap(),
                n(&q["qty"]),
            )
            .unwrap();
            let actual = &q["result"];
            assert_eq!(
                [r.in_left, r.out, r.fee],
                [n(&actual[0]), n(&actual[1]), n(&actual[2])],
                "E3 {} {}",
                p["pool"],
                q
            );
            total += 1;
        }
    }
    assert_eq!(total, 52);
}

#[test]
fn bin_events_reconcile_protocol_fees_and_terminal_wallet_deltas() {
    let d: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!("data/first_run_dex_2026-11-26/e3_base_wallets.json.gz")[..],
    ))
    .unwrap();
    let rows = d["payments"].as_array().unwrap();
    let swap = format!(
        "{:#x}",
        alloy_primitives::keccak256(
            b"Swap(address,address,uint24,bytes32,bytes32,uint24,bytes32,bytes32)"
        )
    );
    for p in rows {
        let mut paid = 0;
        let mut net = 0;
        let mut fee = 0;
        let target = if p["mode"] == "v5" {
            &d["executor"]
        } else {
            &d["router"]
        };
        for log in p["logs"].as_array().unwrap() {
            if log["address"] != p["pool"] || log["topics"][0] != swap {
                continue;
            }
            for (topic, address) in [(1, target), (2, &d["payer"])] {
                assert_eq!(
                    &log["topics"][topic].as_str().unwrap()[26..],
                    &address.as_str().unwrap()[2..]
                );
            }
            let data = log["data"].as_str().unwrap().strip_prefix("0x").unwrap();
            let amount = |index: usize| {
                let word = &data[64 * index..64 * (index + 1)];
                let (y, x) = word.split_at(32);
                let y = u128::from_str_radix(y, 16).unwrap();
                let x = u128::from_str_radix(x, 16).unwrap();
                assert!(x == 0 || y == 0);
                x + y
            };
            paid += amount(1) + amount(5);
            net += amount(2);
            fee += amount(4);
        }
        let raw = |v: &Value| u128::from(v.as_u64().unwrap());
        assert_eq!(
            (paid, net, fee),
            (raw(&p["paid"]), raw(&p["net"]), raw(&p["fee"]))
        );
        assert_eq!(paid, raw(&p["before"][0]) - raw(&p["after"][0]));
        assert_eq!(net, raw(&p["after"][1]) - raw(&p["before"][1]));
    }
    assert_eq!(rows.len(), 16);
}

#[test]
fn funded_cash_limit_and_single_unit_overflow_are_atomic() {
    let d: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!("data/first_run_dex_2026-11-26/e3_base_capacity.json.gz")[..],
    ))
    .unwrap();
    let mut boundaries = std::collections::BTreeSet::new();
    for x in d["capacity"].as_array().unwrap() {
        if x["unfunded"] == true {
            continue;
        }
        let amount = x["amount"].as_u64().unwrap();
        let cap = x["capacity"].as_u64().unwrap();
        assert_eq!(x["success"], amount <= cap);
        if amount > cap {
            assert_eq!(
                (
                    x["left"].as_u64(),
                    x["spent"].as_u64(),
                    x["received"].as_u64()
                ),
                (Some(1), Some(0), Some(0))
            );
        } else {
            assert_eq!(x["spent"], x["amount"]);
            assert_eq!(x["received"], x["nominal"]);
            assert_eq!(x["left"], 0);
        }
        boundaries.insert((
            x["pool"].as_str().unwrap(),
            x["input"].as_str().unwrap(),
            x["mode"].as_str().unwrap(),
            amount.cmp(&cap),
        ));
    }
    assert!(boundaries.len() >= 18);
}
