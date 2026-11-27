//! Исходные квитанции официального Router и защищённого модуля при двух способах оплаты BUN.
use super::bundles_wallet::{Proof, Terms};
use crate::venues::bsc_dex::check::simulate::SimLog;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
#[test]
fn recover_original_debits_and_net_receipts_after_restart() {
    let inputs: [&[u8]; 2] = [
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/bundles_ethereum_wallets.json.gz"
        ),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/bundles_ethereum_protected.json.gz"
        ),
    ];
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let units = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let wallet = "0x730a17276a9720871c98622ffce5db23f38acf99"
        .parse()
        .unwrap();
    for (idx, source) in inputs.into_iter().enumerate() {
        let d: Value = serde_json::from_reader(flate2::read::GzDecoder::new(source)).unwrap();
        let router = addr(&d["router"]["address"]);
        let bun = addr(&d["protocol_token"]);
        let weth = addr(&d["router"]["weth"]);
        let target = if idx == 0 {
            router
        } else {
            addr(&d["module"]["address"])
        };
        let mut count = 0;
        for row in d["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["wire"] != "v5")
        {
            assert_eq!(row["stage"], "settled");
            let m = d["markets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["pool"] == row["pool"])
                .unwrap();
            let token0 = addr(&m["tokens"][0]);
            let input = addr(&row["input"]);
            let output = addr(&m["tokens"][usize::from(token0 == input)]);
            let t = Terms {
                pool: addr(&m["pool"]),
                router,
                target,
                bun,
                weth,
                token0,
                tier: m["fee"].as_u64().unwrap() as u16,
                fee_limit: if row["mode"] == "separate" {
                    units(&row["fee_cap"])
                } else {
                    U256::ZERO
                },
            };
            let min = units(&row["quote"]);
            let qty = units(&row["amount"]);
            let p = Proof {
                terms: t,
                payer: wallet,
                input,
                output,
                amount: qty,
                minimum: min,
                deadline: U256::from(1795643351u64 + 300),
            };
            let mut p: Proof = serde_json::from_value(serde_json::to_value(p).unwrap()).unwrap();
            let raw = row["calldata"].as_str().unwrap().parse::<Bytes>().unwrap();
            assert_eq!(p.data(), raw);
            let logs = serde_json::from_value::<Vec<SimLog>>(row["logs"].clone()).unwrap();
            assert_eq!(
                p.measured(&logs),
                Some((qty, min)),
                "pool {}, amount {} mode {}",
                t.pool,
                qty,
                row["mode"]
            );
            let mut twice = logs.clone();
            twice.extend(logs.clone());
            assert!(p.measured(&twice).is_none());
            p.minimum += U256::ONE;
            assert!(p.measured(&logs).is_none());
            assert!(row["strict"]["code"].is_number() && row["expired"]["code"].is_number());
            count += 1;
        }
        assert_eq!(count, 84);
    }
}
