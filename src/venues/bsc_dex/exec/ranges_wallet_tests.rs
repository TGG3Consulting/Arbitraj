//! Роутер подписывает точный вход и минимум, Vault сообщает именно выбранный пул и токены.
use super::*;
use crate::venues::bsc_dex::check::simulate::SimLog;
use flate2::read::GzDecoder;
use std::collections::HashSet;

#[test]
fn native_router_calldata_and_vault_receipts_survive_restart() {
    let wallet: Value = serde_json::from_reader(GzDecoder::new(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/ranges_ethereum_wallets.json.gz"
        )
        .as_slice(),
    ))
    .unwrap();
    let d: Value = serde_json::from_reader(GzDecoder::new(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/ranges_ethereum_directory.json.gz"
        )
        .as_slice(),
    ))
    .unwrap();
    let exact: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/ranges_ethereum_exact_transfers.json"
    ))
    .unwrap();
    let address = |v: &Value| serde_json::from_value::<Address>(v.clone()).unwrap();
    let tokens: HashSet<_> = exact["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| address(&r["address"]))
        .collect();
    let mut count = 0;
    for r in wallet["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["mode"] == "router")
    {
        let cash = address(&r["cash"]);
        let token = address(&r["token"]);
        if !tokens.contains(&cash) || !tokens.contains(&token) {
            continue;
        }
        let market = Market {
            pool: address(&r["pool"]),
            vault: address(&d["roles"]["vault"]),
            permit: address(&d["roles"]["permit2"]),
        };
        for (side, input, output) in [("buy", cash, token), ("sell", token, cash)] {
            let l = &r[side];
            if l["stage"] != "paid" {
                continue;
            }
            let tx = &l["signed"];
            let data: Bytes = serde_json::from_value(tx["data"].clone()).unwrap();
            let abi = Router::swapSingleTokenExactInCall::abi_decode(&data).unwrap();
            let proof = Proof {
                market,
                input,
                output,
                target: address(&tx["to"]),
                payer: address(&tx["from"]),
                budget: l["debit"].to_string().parse().unwrap(),
                minimum: l["credit"].to_string().parse().unwrap(),
                deadline: abi.deadline,
            };
            assert_eq!(proof.data(), data);
            let logs: Vec<SimLog> = serde_json::from_value(l["logs"].clone()).unwrap();
            let mut restored: Proof =
                serde_json::from_value(serde_json::to_value(&proof).unwrap()).unwrap();
            assert_eq!(
                restored.measured(&logs),
                Some((proof.budget, proof.minimum))
            );
            restored.minimum += U256::ONE;
            assert_eq!(restored.measured(&logs), None);
            restored.minimum = proof.minimum;
            restored.market.pool = proof.target;
            assert_eq!(restored.measured(&logs), None);
            count += 1;
        }
    }
    assert_eq!(count, 28);
}
