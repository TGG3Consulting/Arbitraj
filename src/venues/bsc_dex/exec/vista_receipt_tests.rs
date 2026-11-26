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
                let logs: Vec<_> = result["logs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| alloy::rpc::types::Log {
                        inner: serde_json::from_value(entry.clone()).unwrap(),
                        ..Default::default()
                    })
                    .collect();
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

#[test]
fn rabbit_receipts_recover_the_payers_eth_and_friend_without_counting_router_inventory() {
    let book: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/bunnyswap_base.json"
    ))
    .unwrap();
    for row in book["cases"].as_array().unwrap() {
        for (side, buy) in [("buy", true), ("sell", false)] {
            let leg = &row[side];
            let evidence = &leg["wallet_proof"];
            let weth = addr(&evidence["wrapped"]);
            let friend = addr(&evidence["token"]);
            let wallet = addr(&evidence["wallet"]);
            let data: Bytes =
                serde_json::from_value(evidence["transaction"]["data"].clone()).unwrap();
            let (input, output, form) = if buy {
                (weth, friend, Form::RabbitEthIn)
            } else {
                (friend, weth, Form::RabbitEthOut)
            };
            let p = Proof {
                call: Call {
                    form,
                    wrapped: weth,
                    fee: U256::ZERO,
                },
                target: addr(&book["router"]),
                wallet,
                input,
                output,
                amount: number(&leg["amount"]),
                minimum: number(&leg["received"]),
                data_hash: keccak256(&data),
            };
            assert!(p.signed_data(&data));
            let logs: Vec<_> = evidence["logs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| alloy::rpc::types::Log {
                    inner: serde_json::from_value(v.clone()).unwrap(),
                    ..Default::default()
                })
                .collect();
            let revived: Proof = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
            assert_eq!(
                revived.amounts(&logs, wallet, input, output),
                Some((p.amount, p.minimum))
            );
            assert!(
                revived
                    .amounts(&logs, Address::ZERO, input, output)
                    .is_none()
            );
            assert_eq!(
                p.call.value(p.amount),
                serde_json::from_value::<U256>(evidence["transaction"]["value"].clone()).unwrap()
            );
            let raised = Proof {
                minimum: p.minimum + U256::ONE,
                ..p
            };
            assert!(!raised.signed_data(&data));
            assert!(raised.amounts(&logs, wallet, input, output).is_none());
        }
    }
}
