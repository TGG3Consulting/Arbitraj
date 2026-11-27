//! Проверяем восстановление каждой реальной выплаты Bond, точные границы, чужой получатель и удалённый Transfer.
use super::super::mint_club_wire::{Call, IBondFlow};
use super::Proof;
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, U256};
use flate2::read::GzDecoder;
use serde_json::Value;

fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn number(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn both_networks_match_authorized_quantity_and_actual_transfers_after_restart() {
    let archives = [
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/mint_club_base_wallets.json.gz"
        )
        .as_slice(),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/mint_club_ethereum_wallets.json.gz"
        )
        .as_slice(),
    ];
    let mut n = 0;
    for archive in archives {
        let raw: Value = serde_json::from_reader(GzDecoder::new(archive)).unwrap();
        let bond = address(&raw["bond"]);
        let reserve = address(&raw["weth"]);
        for market in raw["markets"].as_array().unwrap() {
            let token = address(&market["token"]);
            for trial in market["trials"].as_array().unwrap() {
                let qty = number(&trial["size"]);
                let wallet = address(&trial["payer"]);
                for mint in [true, false] {
                    let (cash, key, before, after) = if mint {
                        (
                            number(&trial["mint_cost"]),
                            "mint_logs",
                            "before",
                            "after_mint",
                        )
                    } else {
                        (
                            number(&trial["redeem"]),
                            "burn_logs",
                            "after_mint",
                            "after_burn",
                        )
                    };
                    let mut logs: Vec<Log> = trial[key]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| Log {
                            inner: serde_json::from_value(v.clone()).unwrap(),
                            ..Default::default()
                        })
                        .collect();
                    let c = Call {
                        protection: None,
                        qty,
                        mint,
                        reserve,
                        token,
                        limit: cash,
                    };
                    let wire = c.data(wallet);
                    if mint {
                        let call = IBondFlow::mintCall::abi_decode_validate(&wire).unwrap();
                        assert_eq!(
                            (
                                call.receiver,
                                call.token,
                                call.tokensToMint,
                                call.maxReserveAmount
                            ),
                            (wallet, token, qty, cash)
                        );
                    } else {
                        let call = IBondFlow::burnCall::abi_decode_validate(&wire).unwrap();
                        assert_eq!(
                            (call.receiver, call.token, call.tokensToBurn, call.minRefund),
                            (wallet, token, qty, cash)
                        );
                    }
                    let proof: Proof = serde_json::from_value(
                        serde_json::to_value(Proof {
                            call: c,
                            bond,
                            payer: wallet,
                        })
                        .unwrap(),
                    )
                    .unwrap();
                    let (input, output) = c.pair();
                    let (debit, credit) = if mint { (cash, qty) } else { (qty, cash) };
                    let (di, ci) = if mint { (0, 1) } else { (1, 0) };
                    assert_eq!(
                        number(&trial[before][di]) - number(&trial[after][di]),
                        debit
                    );
                    assert_eq!(
                        number(&trial[after][ci]) - number(&trial[before][ci]),
                        credit
                    );
                    assert_eq!(
                        proof.amounts(&logs, wallet, input, output),
                        Some((debit, credit))
                    );
                    let strict = c.tighten(debit, credit).unwrap();
                    assert_eq!(
                        Proof {
                            call: strict,
                            ..proof.clone()
                        }
                        .amounts(&logs, wallet, input, output),
                        None
                    );
                    assert_eq!(proof.amounts(&logs, Address::ZERO, input, output), None);
                    let event = logs
                        .iter()
                        .find(|l| {
                            l.address() == bond
                                && l.topics().first().is_some_and(|t| {
                                    *t == super::IBond::Mint::SIGNATURE_HASH
                                        || *t == super::IBond::Burn::SIGNATURE_HASH
                                })
                        })
                        .unwrap()
                        .clone();
                    logs.push(event);
                    assert_eq!(proof.amounts(&logs, wallet, input, output), None);
                    logs.pop();
                    logs.retain(|l| l.address() != reserve);
                    assert_eq!(proof.amounts(&logs, wallet, input, output), None);
                    n += 1;
                }
            }
        }
    }
    assert_eq!(n, 34);
}

#[test]
fn protected_wallet_binds_issuer_module_and_receives_each_asset_before_and_after_recovery() {
    use super::super::mint_club_wire::Protection;
    for bytes in [
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/mint_club_base_protected.json.gz"
        )
        .as_slice(),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/mint_club_ethereum_protected.json.gz"
        )
        .as_slice(),
    ] {
        let raw: Value = serde_json::from_reader(GzDecoder::new(bytes)).unwrap();
        let module = address(&raw["module"]);
        let bond = address(&raw["bond"]);
        let wallet = address(&raw["owner"]);
        let cases = raw["cases"]
            .as_array()
            .unwrap()
            .iter()
            .chain(raw["cash_cases"].as_array().into_iter().flatten());
        for row in cases {
            let reserve = row
                .get("reserve")
                .map(address)
                .unwrap_or_else(|| address(&raw["reserve"]));
            let qty = number(&row["qty"]);
            let token = address(&row["token"]);
            for mint in [true, false] {
                let (limit, v) = if mint {
                    (number(&row["cost"]), &row["buy"])
                } else {
                    (number(&row["refund"]), &row["sell"])
                };
                let call = Call {
                    token,
                    reserve,
                    qty,
                    mint,
                    limit,
                    protection: Some(Protection {
                        bond,
                        module,
                        deadline: U256::from(1u8),
                    }),
                };
                let p = Proof {
                    call,
                    bond,
                    payer: wallet,
                };
                let (input, output) = call.pair();
                let logs: Vec<Log> = v["logs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| Log {
                        inner: serde_json::from_value(v.clone()).unwrap(),
                        ..Default::default()
                    })
                    .collect();
                let (debit, credit) = if mint { (limit, qty) } else { (qty, limit) };
                assert_eq!(
                    p.amounts(&logs, wallet, input, output),
                    Some((debit, credit))
                );
                assert_eq!(v["refused"].as_array().unwrap().len(), 4);
                let mut altered = p.clone();
                altered.call.protection.as_mut().unwrap().module = bond;
                assert!(altered.amounts(&logs, wallet, input, output).is_none());
                let mut altered = p.clone();
                altered.call.protection = None;
                assert!(altered.amounts(&logs, wallet, input, output).is_none());
                let data = IBondFlow::executeCall::abi_decode_validate(&call.data(wallet)).unwrap();
                assert_eq!(
                    (data.maxDebit, data.minCredit, data.qty, data.mint),
                    (debit, credit, qty, mint)
                );
            }
        }
    }
}
