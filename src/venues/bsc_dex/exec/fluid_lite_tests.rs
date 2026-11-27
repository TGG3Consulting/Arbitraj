use super::*;
use alloy::{rpc::types::Log, sol_types::SolEvent};
use flate2::read::GzDecoder;
use serde_json::Value;
fn read(bytes: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
}
fn a(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn n(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
/// Снимок DexKey[] и фактический market из calldata архива задают обе стороны 64/192-битной границы.
#[test]
fn recorded_market_id_selects_the_same_directory_key_and_keeps_index_out_of_dex_id() {
    let snapshot = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/fluid_dex_lite_ethereum.json.gz"
    ));
    let wallet = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/fluid_dex_lite_wallet.json.gz"
    ));
    assert_eq!(wallet["head"], snapshot["active"]["head"]);
    let directory = snapshot["active"]["pools"].as_array().unwrap();
    let mut selected = std::collections::BTreeSet::new();
    for payment in wallet["payments"].as_array().unwrap() {
        let id = payment["market"].as_str().unwrap().parse::<B256>().unwrap();
        let index = U256::from_be_slice(&id[8..]);
        let position = usize::try_from(index).unwrap();
        let row = &directory[position];
        let market = Market {
            token0: a(&row["token0"]),
            token1: a(&row["token1"]),
            salt: row["salt"].as_str().unwrap().parse().unwrap(),
            index,
        };
        assert_eq!(market.id(), Some(id));
        assert_eq!(format!("0x{}", hex::encode(&id[..8])), row["pool_id"]);
        if !selected.insert(position) {
            continue;
        }

        // Отдельная граница формата PoolRef. Такой индекс не заявляется существующим в каталоге.
        let overflow: U256 = U256::ONE << 192;
        let largest = Market {
            index: overflow - U256::ONE,
            ..market
        }
        .id()
        .unwrap();
        assert_eq!(&largest[..8], &id[..8]);
        assert_eq!(&largest[8..], &[u8::MAX; 24]);
        assert_eq!(
            Market {
                index: overflow,
                ..market
            }
            .id(),
            None
        );
    }
    assert_eq!(selected.len(), directory.len());
    assert!(selected.len() > 1);
}

#[test]
fn two_physical_markets_and_twelve_paid_wallet_routes() {
    let raw = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/fluid_dex_lite_ethereum.json.gz"
    ));
    let wallet = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/fluid_dex_lite_wallet.json.gz"
    ));
    assert_eq!(wallet["head"], raw["active"]["head"]);
    let rows = raw["active"]["pools"].as_array().unwrap();
    let reference = raw["reference"]["pools"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.len(), reference.len());
    let owner = a(&wallet["owner"]);
    let dex = a(&wallet["manifest"]["exchange"]);
    let wrapped = a(&wallet["manifest"]["wrapped"]);
    let module = a(&wallet["module"]);
    let markets: Vec<_> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let market = Market {
                token0: a(&row["token0"]),
                token1: a(&row["token1"]),
                salt: row["salt"].as_str().unwrap().parse().unwrap(),
                index: U256::from(i),
            };
            let id = market.id().unwrap();
            assert_eq!(
                &id[..8],
                hex::decode(&row["pool_id"].as_str().unwrap()[2..])
                    .unwrap()
                    .as_slice()
            );
            assert_eq!(reference[i]["pool_id"], row["pool_id"]);
            assert!(
                reference[i]["payments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|v| v["funded_error"].is_object())
            );
            assert!(
                row["payments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|v| !n(&v["received"]).is_zero() && !n(&v["spent"]).is_zero())
            );
            market
        })
        .collect();
    let mut totals = [0; 2];
    for trade in wallet["payments"].as_array().unwrap() {
        let key = trade["market"].as_str().unwrap().parse::<B256>().unwrap();
        let market = *markets.iter().find(|m| m.id() == Some(key)).unwrap();
        let logs: Vec<Log> = serde_json::from_value(trade["logs"].clone()).unwrap();
        let input = a(&trade["source"]);
        let output = a(&trade["target"]);
        let spent = n(&trade["spent"]);
        let net = n(&trade["received"]);
        assert!(trade["strict"].is_object());
        assert!(!trade["controls"].as_array().unwrap().is_empty());
        if trade["mode"] == "direct" {
            let calldata = trade["tx"]["data"]
                .as_str()
                .unwrap()
                .parse::<Bytes>()
                .unwrap();
            let wire = abi::IWallet::executeCall::abi_decode(&calldata).unwrap();
            let proof = Proof {
                route: None,
                payer: owner,
                input,
                output,
                maximum: wire.budget,
                minimum: wire.minimum,
                deadline: wire.deadline,
                terms: Terms {
                    market,
                    target: module,
                    wrapped,
                    dex,
                    desired: wire.exactOut.then_some(wire.minimum),
                    module_hash: wallet["manifest"]["runtimeKeccak"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap(),
                },
            };
            assert_eq!(proof.data(), calldata);
            assert_eq!(
                proof.amounts(&logs, owner, input, output),
                Some((spent, net))
            );
            let mut wrong = proof.clone();
            wrong.terms.market.salt = B256::repeat_byte(0x81);
            assert!(wrong.amounts(&logs, owner, input, output).is_none());
            let mut duplicated = logs.clone();
            duplicated.push(
                logs.iter()
                    .find(|l| {
                        l.address() == dex
                            && l.topic0() == Some(&abi::IWallet::LogSwap::SIGNATURE_HASH)
                    })
                    .unwrap()
                    .clone(),
            );
            assert!(proof.amounts(&duplicated, owner, input, output).is_none());
            totals[0] += 1;
        } else {
            let event = logs
                .iter()
                .find(|l| {
                    l.address() == module
                        && l.topic0() == Some(&abi::IWallet::Filled::SIGNATURE_HASH)
                })
                .unwrap();
            let paid = abi::IWallet::Filled::decode_log_data(&event.inner.data).unwrap();
            assert_eq!(
                (
                    paid.market,
                    paid.payer,
                    paid.recipient,
                    paid.spent,
                    paid.received
                ),
                (key, a(&trade["tx"]["to"]), owner, spent, net)
            );
            totals[1] += 1;
        }
    }
    assert_eq!(totals, [8, 4]);
}

#[test]
fn ordered_hops_recover_both_directions_and_a_repeated_pool() {
    use super::super::fluid_lite_hop::Route;
    let rows = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/fluid_dex_lite_hops.json.gz"
    ));
    let standalone = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/fluid_dex_lite_ethereum.json.gz"
    ));
    let wallet = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/fluid_dex_lite_wallet.json.gz"
    ));
    assert_eq!(rows["head"], wallet["head"]);
    let registered: Vec<_> = standalone["active"]["pools"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, p)| Market {
            token0: a(&p["token0"]),
            token1: a(&p["token1"]),
            salt: p["salt"].as_str().unwrap().parse().unwrap(),
            index: U256::from(index),
        })
        .collect();
    let payments = rows["payments"].as_array().unwrap();
    assert_eq!(payments.len(), 9);
    let mut lengths = vec![];
    for triple in payments.chunks_exact(3) {
        let row = triple.iter().find(|v| v["form"] == "protected").unwrap();
        assert!(triple.iter().all(|v| v["spent"] == row["spent"]
            && v["received"] == row["received"]
            && v["strict"].is_object()));
        assert!(
            row["gas"].as_u64()
                < triple.iter().find(|v| v["form"] == "v5").unwrap()["gas"].as_u64()
        );
        let calldata: Bytes = row["tx"]["data"].as_str().unwrap().parse().unwrap();
        let wire = abi::IWallet::executeHopCall::abi_decode(&calldata).unwrap();
        let markets: Vec<_> = wire
            .markets
            .iter()
            .map(|k| *registered.iter().find(|m| m.id() == Some(*k)).unwrap())
            .collect();
        let terms = Terms {
            market: markets[0],
            dex: a(&wallet["manifest"]["exchange"]),
            wrapped: a(&wallet["manifest"]["wrapped"]),
            target: a(&row["tx"]["to"]),
            module_hash: wallet["manifest"]["runtimeKeccak"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            desired: None,
        };
        let payer = a(&row["tx"]["from"]);
        let input = a(&row["source"]);
        let output = a(&row["target"]);
        let probe = Proof {
            terms,
            route: Some(Route {
                markets,
                path: wire.path,
            }),
            payer,
            input,
            output,
            maximum: wire.amountIn,
            minimum: wire.minimum,
            deadline: wire.deadline,
        };
        assert_eq!(probe.data(), calldata);
        let logs: Vec<Log> = serde_json::from_value(row["logs"].clone()).unwrap();
        assert_eq!(
            probe.amounts(&logs, payer, input, output),
            Some((n(&row["spent"]), n(&row["received"])))
        );
        let recovered: Proof =
            serde_json::from_value(serde_json::to_value(&probe).unwrap()).unwrap();
        assert_eq!(recovered.data(), calldata);
        let mut reordered = logs.clone();
        let indexes: Vec<_> = logs
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.address() == terms.dex
                    && l.topic0() == Some(&abi::IWallet::LogSwap::SIGNATURE_HASH)
            })
            .map(|(i, _)| i)
            .collect();
        reordered.swap(indexes[0], *indexes.last().unwrap());
        assert_eq!(recovered.amounts(&reordered, payer, input, output), None);
        lengths.push(indexes.len());
    }
    assert_eq!(lengths, vec![2, 2, 4]);
}
