use super::aqua_wallet::{Proof, Terms};
use crate::venues::bsc_dex::pools::aqua::{self, IAqua, Program};
use alloy::{
    rpc::types::Log,
    sol_types::{SolEvent, SolValue},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use flate2::read::GzDecoder;
use serde_json::Value;

fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
}
fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn amount(v: &Value) -> U256 {
    v.to_string().parse().unwrap()
}

fn wallet_rows(day: &Value, source: &Value, chain: u64) -> usize {
    let mut count = 0;
    let books = source["strategies"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(source));
    for book in books.iter().filter(|b| b["mode"] != "v5") {
        let payer = address(&book["wallet"]);
        let engine = address(&book["router"]);
        let target = book["protected"]["module"]
            .as_str()
            .map(|s| s.parse().unwrap())
            .unwrap_or(engine);
        let h = book["order"].as_str().unwrap();
        let hash: B256 = h.parse().unwrap();
        let o = &day["orders"][h];
        let program = Program {
            maker: address(&o["maker"]),
            traits: o["traits"].as_str().unwrap().parse().unwrap(),
            data: o["data"].as_str().unwrap().parse().unwrap(),
        };
        let raw = Bytes::from(program.abi_encode());
        assert!(aqua::program(program.maker, hash, &raw).is_some());
        assert!(aqua::program(program.maker, B256::ZERO, &raw).is_none());
        for r in book["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| !r["paid"].is_null())
        {
            let input = address(&r["input"]);
            let output = address(&r["output"]);
            let paid = amount(&r["paid"]);
            let net = amount(&r["net"]);
            let first = r["flags"].as_u64().unwrap() == 0x61;
            let logs: Vec<Log> = serde_json::from_value(r["logs"].clone()).unwrap();
            let proof = Proof {
                terms: Terms {
                    chain,
                    target,
                    engine,
                    maker: program.maker,
                    hash,
                    raw: raw.clone(),
                    first,
                },
                payer,
                input,
                output,
                amount: paid,
                minimum: net,
                deadline: U256::from(day["timestamp"].as_u64().unwrap() + 1000),
            };
            assert!(!proof.data().is_empty());
            assert_eq!(
                proof.amounts(&logs, payer, input, output),
                Some((paid, net))
            );
            let mut stricter = proof.clone();
            stricter.minimum += U256::ONE;
            assert_eq!(stricter.amounts(&logs, payer, input, output), None);
            assert_eq!(proof.amounts(&logs, Address::ZERO, input, output), None);
            let swap = logs
                .iter()
                .find(|l| {
                    l.address() == engine && l.topic0() == Some(&IAqua::Swapped::SIGNATURE_HASH)
                })
                .unwrap();
            let pair = (input.min(output), input.max(output));
            let id = aqua::edge(hash, pair, first);
            let reference = crate::core::types::PoolRef::Singleton {
                manager: engine,
                id,
            };
            let mut event = crate::venues::bsc_dex::check::simulate::SimLog {
                address: engine,
                topics: swap.topics().to_vec(),
                data: swap.inner.data.data.clone(),
            };
            assert!(aqua::emitted(&reference, pair, &event));
            assert!(!aqua::emitted(
                &crate::core::types::PoolRef::Singleton {
                    manager: engine,
                    id: B256::ZERO
                },
                pair,
                &event
            ));
            assert!(!aqua::emitted(&reference, (Address::ZERO, output), &event));
            event.address = Address::ZERO;
            assert!(!aqua::emitted(&reference, pair, &event));
            let mut duplicates = logs.clone();
            duplicates.push(swap.clone());
            assert_eq!(proof.amounts(&duplicates, payer, input, output), None);
            assert!(
                r["rejects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|x| x["code"] == 3)
            );
            count += 1;
        }
    }
    count
}

#[test]
fn receipts_follow_the_actual_wallet_and_the_signed_program() {
    macro_rules! data {
        ($f:literal) => {
            archive(include_bytes!(concat!(
                "../../../../tests/data/first_run_dex_2026-11-26/",
                $f,
                ".json.gz"
            )))
        };
    }
    let base = data!("aqua_base_day");
    let eth = data!("aqua_ethereum_day");
    assert_eq!(wallet_rows(&base, &data!("aqua_base_wallets"), 8453), 4);
    assert_eq!(wallet_rows(&eth, &data!("aqua_ethereum_wallets"), 1), 32);
    assert_eq!(wallet_rows(&base, &data!("aqua_base_protected"), 8453), 4);
    assert_eq!(wallet_rows(&eth, &data!("aqua_ethereum_protected"), 1), 32);
}
