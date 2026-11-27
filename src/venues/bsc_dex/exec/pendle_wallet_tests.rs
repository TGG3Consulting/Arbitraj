//! Квитанции модуля Pendle, физическая оплата PT/SY и сохранённый минимум.
use super::pendle_wallet::{IAdapter, Proof};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
fn addr(v: &Value) -> Address {
    serde_json::from_value(v.clone()).unwrap()
}
fn number(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
#[test]
fn accepted_wallet_orders_bind_all_assets_sender_and_deadline() {
    for (raw, expected) in [
        (include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/pendle_ethereum_protected_wallets.json.gz").as_slice(), [10,9,10,10,0]),
        (include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/pendle_ethereum_chain_bound_wallets.json.gz").as_slice(), [10,9,10,10,0]),
        (include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/pendle_base_protected_wallets.json.gz").as_slice(), [4,0,4,4,4]),
    ] {
        check_receipts(raw, expected);
    }
}

fn check_receipts(raw: &[u8], expected: [usize; 5]) {
    let archive: Value = serde_json::from_reader(flate2::read::GzDecoder::new(raw)).unwrap();
    let data = &archive["module"];
    let mut counts = [0; 5];
    for c in data["cases"].as_array().unwrap() {
        for (k, row) in c.as_object().unwrap() {
            let i = match k.as_str() {
                "buy" => 0,
                "sell" => 1,
                "sell_to_sy" => 2,
                "buy_with_sy" => 3,
                name if name.starts_with("redeem_") => 4,
                _ => continue,
            };
            if row["stage"] != "accepted" {
                continue;
            }
            let wire: Bytes = serde_json::from_value(row["transaction"]["data"].clone()).unwrap();
            let tx = IAdapter::executeCall::abi_decode(&wire).unwrap();
            let logs: Vec<crate::venues::bsc_dex::check::simulate::SimLog> =
                serde_json::from_value(row["logs"].clone()).unwrap();
            let mut saved = Proof {
                market: addr(&c["market"]),
                module: addr(&data["module"]),
                payer: addr(&data["payer"]),
                input: addr(&row["input"]),
                output: addr(&row["output"]),
                amount: number(&row["paid"]),
                minimum: number(&row["net"]),
                deadline: tx.deadline,
            };
            assert_eq!(saved.data(), wire);
            assert_eq!(saved.measured(&logs), Some((saved.amount, saved.minimum)));
            assert!(row["strict"].is_object() && row["expiry_error"].is_object());
            saved.minimum += U256::ONE;
            assert_eq!(saved.measured(&logs), None);
            saved.minimum -= U256::ONE;
            saved.market = saved.module;
            assert_eq!(saved.measured(&logs), None);
            counts[i] += 1;
            let inventory = c["router_inventory"][k.as_str()].as_array().unwrap();
            assert_eq!(inventory[0], inventory[1]);
        }
    }
    assert_eq!(counts, expected);
}
