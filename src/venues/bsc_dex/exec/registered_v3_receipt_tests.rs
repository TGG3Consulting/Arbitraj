//! Квитанции Router с двух фабрик и границы исходного сохранённого поручения.
use super::*;
use crate::venues::bsc_dex::check::simulate::SimLog;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn wallet_limits_and_identity_survive_serialization() {
    let proof: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/ethereum_exact_transfers.json"
    ))
    .unwrap();
    let exact: Vec<Address> = proof["tokens"]
        .as_object()
        .unwrap()
        .values()
        .map(|r| r["address"].as_str().unwrap().parse().unwrap())
        .collect();
    for raw in [
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/wagmi_ethereum_wallets.json.gz"
        )
        .as_slice(),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/lif3_ethereum_wallets.json.gz"
        )
        .as_slice(),
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/blueprint_ethereum_wallets.json.gz").as_slice(),
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/vvs_ethereum_wallets.json.gz").as_slice(),
    ] {
        let archive: Value = serde_json::from_reader(GzDecoder::new(raw)).unwrap();
        let parse = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let extended=archive["module_hash"].is_string();
        let mut allowed=exact.clone();
        if extended {
            let directory:Value=serde_json::from_reader(GzDecoder::new(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/vvs_ethereum_directory.json.gz").as_slice())).unwrap();
            allowed.push(parse(&directory["exact_token"]["address"]));
        }
        let mut count = 0;
        for row in archive["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["wire"] == "router")
        {
            let input = parse(&row["source"]);
            let output = parse(&row["target"]);
            if !allowed.contains(&input) || !allowed.contains(&output) {
                continue;
            }
            let pool = parse(&row["pool"]);
            let target = parse(&archive["roles"]["router"]["address"]);
            let payer: Address = "0x730a17276a9720871c98622ffce5db23f38acf99"
                .parse()
                .unwrap();
            let spent: U256 = row["spent"].as_str().unwrap().parse().unwrap();
            let received: U256 = row["received"].as_str().unwrap().parse().unwrap();
            let node = archive["markets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["pool"] == row["pool"])
                .unwrap();
            let market = Market {
                pool,
                fee_event:extended,wallet_module:false,
                fee: U24::from(node["fee"].as_u64().unwrap() as u32),
            };
            let bound = Proof {
                market,
                target,
                payer,
                input,
                output,
                budget: spent,
                minimum: received,
                deadline: U256::from(1000),
            };
            assert_eq!(serde_json::to_value(market).unwrap().get("fee_event").is_some(),extended);
            let mut proof: Proof =
                serde_json::from_value(serde_json::to_value(&bound).unwrap()).unwrap();
            assert_eq!(bound.data(), proof.data());
            let logs: Vec<SimLog> = serde_json::from_value(row["logs"].clone()).unwrap();
            assert_eq!(proof.measured(&logs), Some((spent, received)));
            // Если на границе диапазона исполнилась часть поручения, расход остаётся фактическим.
            proof.budget += U256::ONE;
            assert_eq!(proof.measured(&logs), Some((spent, received)));
            proof.minimum += U256::ONE;
            assert!(proof.measured(&logs).is_none());
            proof.minimum = received;
            proof.budget = spent - U256::ONE;
            assert!(proof.measured(&logs).is_none());
            proof.budget = spent;
            proof.target = payer;
            assert!(proof.measured(&logs).is_none());
            proof.target = target;
            proof.payer = target;
            assert!(proof.measured(&logs).is_none());
            proof.payer = payer;
            let mut duplicated = logs.clone();
            duplicated.push(
                logs.iter()
                    .find(|l| {
                        l.address == pool
                            && l.topics.first()
                                == Some(&keccak256(market.event().0))
                    })
                    .unwrap()
                    .clone(),
            );
            assert!(proof.measured(&duplicated).is_none());
            count += 1;
        }
        assert!(count > 0);
    }
}
