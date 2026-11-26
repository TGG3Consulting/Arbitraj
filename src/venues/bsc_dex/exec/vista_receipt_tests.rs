//! Регрессия восстановления на оригинальных вызовах и событиях архивной EVM Base и Ethereum.
use super::vista_fact::Proof;
use super::vista_wire::{Call, Form};
use crate::core::types::Side;
use alloy_primitives::{Address, Bytes, U256, keccak256};
use flate2::read::GzDecoder;
use serde_json::Value;

fn number(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn addr(v: &Value) -> Address {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn confirmed_native_wallet_fill_checks_original_recipient_input_and_terminal_minimum() {
    let snapshots: [&[u8]; 2] = [
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/ethervista_v5_base.json.gz"
        ),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/ethervista_v5_ethereum.json.gz"
        ),
    ];
    for bytes in snapshots {
        let archive: Vec<Value> = serde_json::from_reader(GzDecoder::new(bytes)).unwrap();
        let row = archive
            .iter()
            .find(|r| r["native"].as_array().is_some_and(|v| !v.is_empty()))
            .unwrap();
        let wrapped = addr(&row["path"][1]);
        let owner = addr(&row["recipient"]);
        for case in row["native"].as_array().unwrap() {
            let cash = addr(&case["cash"]);
            let size = number(&case["budget"]);
            let coins = number(&case["buy_module"]["settled"][0]["received"]);
            let fee = number(&case["sell_fee"]);
            let mut checks = vec![(
                "buy_router",
                Form::OfficialEthIn,
                Side::Sell,
                wrapped,
                cash,
                size,
                coins,
                number(&case["buy_fee"]),
            )];
            if let Ok(net) = case["expected_sell_net"].as_str().unwrap().parse::<U256>() {
                checks.push((
                    "sell_router_self",
                    Form::OfficialEthOut,
                    Side::Buy,
                    cash,
                    wrapped,
                    coins,
                    net,
                    fee,
                ));
                checks.push((
                    "sell_module_self",
                    Form::ModuleEthOut,
                    Side::Buy,
                    cash,
                    wrapped,
                    coins,
                    net,
                    fee,
                ));
            }
            for (key, form, side, input, output, amount, minimum, fee) in checks {
                let result = &case[key];
                assert_eq!(result["status"], "0x1");
                let tx = &result["transaction"];
                let data: Bytes = serde_json::from_value(tx["data"].clone()).unwrap();
                let params = Call { form, wrapped, fee };
                let p = Proof {
                    call: params,
                    target: addr(&tx["to"]),
                    wallet: owner,
                    input,
                    output,
                    amount,
                    minimum,
                    data_hash: keccak256(&data),
                };
                assert_eq!(addr(&tx["from"]), owner);
                assert_eq!(
                    serde_json::from_value::<U256>(tx["value"].clone()).unwrap(),
                    p.call.value(amount)
                );
                assert!(p.signed_data(&data));
                // eth_simulateV1 сохраняет адрес и содержимое события без полей добытой квитанции.
                let logs: Vec<_> = result["logs"].as_array().unwrap().iter().map(|entry| {
                    alloy::rpc::types::Log {
                        inner: serde_json::from_value(entry.clone()).unwrap(),
                        ..Default::default()
                    }
                }).collect();
                assert_eq!(
                    p.amounts(&logs, owner, input, output),
                    Some((amount, minimum))
                );
                let revived: Proof =
                    serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
                assert!(revived.signed_data(&data));
                assert_eq!(
                    revived.amounts(&logs, owner, input, output),
                    Some((amount, minimum))
                );
                if side == Side::Buy {
                    let foreign = &case[if form == Form::OfficialEthOut {
                        "sell_router"
                    } else {
                        "sell_module"
                    }]["transaction"];
                    let foreign_data: Bytes =
                        serde_json::from_value(foreign["data"].clone()).unwrap();
                    let wrong = Proof {
                        data_hash: keccak256(&foreign_data),
                        ..p.clone()
                    };
                    assert!(!wrong.signed_data(&foreign_data));
                }
            }
        }
    }
}
