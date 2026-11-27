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

#[test]
fn surge_native_recovers_raw_ether_and_token_payments_from_the_signed_market() {
    use alloy::sol_types::SolCall;
    let d: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/surge_ethereum_wallets.json.gz"
        )[..],
    ))
    .unwrap();
    let token = addr(&d["address"]);
    let payer = addr(&d["sender"]);
    let weth = Address::repeat_byte(2);
    for row in d["cases"].as_array().unwrap() {
        for (side, form, input, output, amount, paid) in [
            (
                "buy",
                Form::SurgeEthIn,
                weth,
                token,
                number(&row["input"]),
                number(&row["bought"]),
            ),
            (
                "sell",
                Form::SurgeEthOut,
                token,
                weth,
                number(&row["bought"]),
                number(&row["received"]),
            ),
        ] {
            let raw = &row[side];
            let data: Bytes = serde_json::from_value(raw["calldata"]["data"].clone()).unwrap();
            // The historical seller specified a gross minimum. Construct the smallest guaranteed
            // wallet floor which encodes that exact signed minimum under the audited 90% bound.
            let minimum = if side == "buy" {
                paid
            } else {
                let gross = super::vista_wire::IVistaFlow::_sellCall::abi_decode(&data)
                    .unwrap()
                    .grossMinimum;
                // Some gross values cannot be emitted by the conservative floor. The receipt still proves its original net.
                (gross * U256::from(90)).div_ceil(U256::from(100))
            };
            let p = Proof {
                call: Call {
                    form,
                    wrapped: weth,
                    fee: U256::ZERO,
                },
                target: token,
                wallet: payer,
                input,
                output,
                amount,
                minimum,
                data_hash: keccak256(&data),
            };
            let logs: Vec<_> = raw["logs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| alloy::rpc::types::Log {
                    inner: serde_json::from_value(l.clone()).unwrap(),
                    ..Default::default()
                })
                .collect();
            assert_eq!(p.amounts(&logs, payer, input, output), Some((amount, paid)));
            assert_eq!(p.amounts(&logs, token, input, output), None);
            let d = super::vista_wire::IVistaFlow::_sellCall::abi_decode(&data)
                .map(|a| a.deadline)
                .or_else(|_| {
                    super::vista_wire::IVistaFlow::_buyCall::abi_decode(&data).map(|a| a.deadline)
                })
                .unwrap();
            let signed = p.call.data(input, output, amount, p.minimum, payer, d);
            let mut restored: Proof =
                serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
            restored.data_hash = keccak256(&signed);
            assert!(restored.signed_data(&signed));
            assert_eq!(
                restored.amounts(&logs, payer, input, output),
                Some((amount, paid))
            );
        }
    }
}

#[test]
fn surge_protected_wallet_recovers_exact_transfers_and_keeps_v5_cash_units() {
    let d: Value = serde_json::from_reader(GzDecoder::new(&include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/surge_ethereum_protected_wallets.json.gz"
    )[..])).unwrap();
    let wallet = addr(&d["payer"]);
    let wrapped = addr(&d["wrapped"]);
    let market = addr(&d["market"]);
    let mut legs = [0usize; 3];
    for case in d["cases"].as_array().unwrap() {
        let cash = addr(&case["cash"]);
        assert_eq!(case["buy"]["after"], case["sell"]["before"]);
        assert!(
            case["same_block"]["message"]
                .as_str()
                .unwrap()
                .contains("same block")
        );
        for (name, source, dest) in [("buy", cash, market), ("sell", market, cash)] {
            let paid = &case[name];
            let before = paid["before"].as_array().unwrap();
            let after = paid["after"].as_array().unwrap();
            assert_eq!(&before[2..], &after[2..]);
            let (i, o) = if name == "buy" { (0, 1) } else { (1, 0) };
            let amount = number(&paid["spent"]);
            let minimum = number(&paid["received"]);
            assert_eq!(number(&before[i]) - number(&after[i]), amount);
            assert_eq!(number(&after[o]) - number(&before[o]), minimum);
            assert!(
                paid["failures"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|f| f["code"] == 3)
            );
            assert_eq!(paid["failures"].as_array().unwrap().len(), 2);
            let t = &paid["calldata"];
            let direct = case["mode"] == "module";
            let target = addr(&t["to"]);
            assert_eq!(
                target,
                addr(&d[if direct { "modeled_module" } else { "executor" }])
            );
            assert_eq!(addr(&t["from"]), wallet);
            if direct {
                legs[0] += 1;
                let data: Bytes = serde_json::from_value(t["data"].clone()).unwrap();
                let p = Proof {
                    call: Call {
                        form: Form::SurgeToken,
                        wrapped,
                        fee: U256::ZERO,
                    },
                    target,
                    wallet,
                    input: source,
                    output: dest,
                    amount,
                    minimum,
                    data_hash: keccak256(&data),
                };
                let logs: Vec<_> = paid["logs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| alloy::rpc::types::Log {
                        inner: serde_json::from_value(l.clone()).unwrap(),
                        ..Default::default()
                    })
                    .collect();
                let restored: Proof =
                    serde_json::from_value(serde_json::to_value(p).unwrap()).unwrap();
                assert!(restored.signed_data(&data));
                assert_eq!(
                    restored.amounts(&logs, wallet, source, dest),
                    Some((amount, minimum))
                );
                let wrong = Proof {
                    minimum: minimum + U256::ONE,
                    ..restored
                };
                assert!(!wrong.signed_data(&data));
                assert!(wrong.amounts(&logs, wallet, source, dest).is_none());
            } else {
                legs[if cash == wrapped { 1 } else { 2 }] += 1;
            }
        }
    }
    assert_eq!(legs, [4, 4, 4]);
}
