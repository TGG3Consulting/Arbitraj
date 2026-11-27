//! Проверка подписанного вызова и оплаты по денежным архивам Ethereum.
use super::*;
use serde_json::Value;

const MULTI: &[u8] = include_bytes!(
    "../../../../tests/data/first_run_dex_2026-11-26/defiplaza_multi_protected_ethereum.json.gz"
);
const STABLE: &[u8] = include_bytes!(
    "../../../../tests/data/first_run_dex_2026-11-26/defiplaza_stable_protected_ethereum.json.gz"
);

fn addr(value: &Value) -> Address {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn canonical_wire_and_terminal_transfers_match_both_generations_and_currencies() {
    let wrapped = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
        .parse()
        .unwrap();
    for (bytes, stable) in [(MULTI, false), (STABLE, true)] {
        let book: Value = serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap();
        let payer = addr(&book["payer"]);
        let direct: Vec<_> = book["fills"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["mode"] != "v5")
            .collect();
        assert_eq!(direct.len(), 12);
        for row in direct {
            let data: Bytes = row["action"]["data"].as_str().unwrap().parse().unwrap();
            let wire = abi::IModule::executeCall::abi_decode_validate(&data).unwrap();
            let normalized = |a: Address| if a.is_zero() { wrapped } else { a };
            let proof = Proof {
                terms: Terms {
                    pool: addr(&book["pool"]),
                    target: addr(&book["module"]),
                    wrapped,
                    native: row["mode"] == "native",
                    stable,
                    module_hash: book["runtime"].as_str().unwrap().parse().unwrap(),
                },
                payer,
                input: normalized(wire.input),
                output: normalized(wire.output),
                maximum: wire.amount,
                minimum: wire.minimum,
                deadline: wire.deadline,
            };
            assert_eq!(proof.data(), data);
            let logs: Vec<Log> = serde_json::from_value(row["logs"].clone()).unwrap();
            assert_eq!(
                proof.amounts(&logs, payer, proof.input, proof.output),
                Some((proof.maximum, proof.minimum))
            );
            assert!(
                proof
                    .amounts(&logs, Address::ZERO, proof.input, proof.output)
                    .is_none()
            );
            let mut duplicate = logs.clone();
            duplicate.extend_from_slice(&logs);
            assert!(
                proof
                    .amounts(&duplicate, payer, proof.input, proof.output)
                    .is_none()
            );
            for rejected in ["zero_min", "plus_one", "expiry"] {
                assert!(row[rejected].is_object());
            }
        }
    }
}
