//! Исчерпаны исходные поручения PMM за 7200 Ethereum-блоков. Входы истекли; тест читает только архивы.
use super::*;
use crate::venues::bsc_dex::tokenlon::wire::{
    self, Prepared, USER_PROXY,
    abi::{fillCall, fillOrKillOrderCall, toPMMCall},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Bytes, keccak256};
use flate2::read::GzDecoder;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Read};

pub(super) fn archive(bytes: &[u8]) -> Value {
    let mut raw = Vec::new();
    GzDecoder::new(bytes).read_to_end(&mut raw).unwrap();
    serde_json::from_slice(&raw).unwrap()
}
fn num(s: &Value) -> U256 {
    s.as_str().unwrap().parse().unwrap()
}
pub(super) fn logs(data: &Value) -> Vec<Log> {
    data.as_array()
        .unwrap()
        .iter()
        .map(|l| Log {
            inner: serde_json::from_value(l.clone()).unwrap(),
            ..Default::default()
        })
        .collect()
}
fn api(o: &fillOrKillOrderCall, t: &Terms) -> Value {
    let a = &o.order;
    json!({"protocol":"PMMV5","makerAddress":a.makerAddress,"takerAddress":a.takerAddress,
        "senderAddress":a.senderAddress,"feeRecipientAddress":a.feeRecipientAddress,"makerFee":a.makerFee.to_string(),
        "takerFee":a.takerFee.to_string(),"salt":a.salt.to_string(),"feeFactor":(a.salt & U256::from(u16::MAX)).to::<u16>(),
        "makerAssetAddress":t.output,"takerAssetAddress":t.input,"makerAssetData":a.makerAssetData,"takerAssetData":a.takerAssetData,
        "makerAssetAmount":a.makerAssetAmount.to_string(),"takerAssetAmount":a.takerAssetAmount.to_string(),
        "makerWalletSignature":o.signature,"expirationTimeSeconds":a.expirationTimeSeconds.to_string(),"exchangeAddress":ZERO_EXCHANGE})
}

#[test]
fn expired_tokenlon_orders_match_101_receipts_and_eight_balance_snapshots() {
    let history = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_expired_pmm.json.gz"
    ));
    let movements = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_wallets.json.gz"
    ));
    let sites = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_source_events.json.gz"
    ));
    let mut makers = BTreeMap::new();
    let (mut controls, mut unchanged, mut external_sender) = (0, 0, 0);
    for row in history["rows"].as_array().unwrap() {
        let tx = &row["tx"];
        let data: Bytes = tx["input"].as_str().unwrap().parse().unwrap();
        let logs = logs(&row["logs"]);
        let pay = unique::<FillOrder>(&logs, PMM).unwrap();
        let at = row["timestamp"].as_u64().unwrap();
        let terms = Terms {
            payer: pay.userAddr,
            recipient: pay.receiverAddr,
            input: pay.takerAssetAddr,
            output: pay.makerAssetAddr,
            spent: pay.takerAssetAmount,
            minimum: pay.settleAmount,
        };
        let expected = Prepared::from_calldata(&data, &terms, at).unwrap();
        let proof = &expected.proof;
        let outer = toPMMCall::abi_decode(&data).unwrap();
        let inner = fillCall::abi_decode(&outer.payload).unwrap();
        let order = fillOrKillOrderCall::abi_decode(&inner.data).unwrap();
        // SDK-конверт восстановлен из публичной истёкшей on-chain заявки. Портфель бота её не подписывал.
        let raw = api(&order, &terms);
        let sig = &inner.userSignature;
        let signature = alloy_primitives::Signature::new(
            U256::from_be_slice(&sig[1..33]),
            U256::from_be_slice(&sig[33..65]),
            sig[0] == 28,
        );
        if terms.recipient == terms.payer {
            let fresh = wire::prepare(&raw, &terms, inner.userSalt, at).unwrap();
            assert_eq!(&fresh.proof, proof);
            assert_eq!(fresh.sign_digest, expected.sign_digest);
            assert_eq!(fresh.calldata(&signature).unwrap(), data);
        } else {
            // 0x9fe61…c80e7a: кошелёк подписал передачу 489.883454 USDC другому получателю.
            assert_eq!(
                tx["hash"],
                "0x9fe61c706bfbcebf2cff2cfa828ab0e75e14b38f5f1d3f1efc1a3decb8c80e7a"
            );
            assert_eq!(pay.settleAmount, U256::from(489883454u64));
            assert!(wire::prepare(&raw, &terms, inner.userSalt, at).is_none());
            assert!(expected.calldata(&signature).is_none());
        }
        assert!(wire::prepare(&raw, &terms, inner.userSalt, proof.expiry).is_none());
        let high = Terms {
            minimum: terms.minimum + U256::ONE,
            ..terms.clone()
        };
        assert!(Prepared::from_calldata(&data, &high, at).is_none());
        let (spent, received) = proof.amounts(&logs).unwrap();
        assert_eq!((spent, received), (terms.spent, pay.settleAmount));
        assert_eq!(
            tx["to"].as_str().unwrap().parse::<Address>().unwrap(),
            USER_PROXY
        );
        assert_eq!(
            serde_json::from_value::<U256>(tx["value"].clone()).unwrap(),
            proof.value()
        );
        assert_eq!(row["canonical"], tx["hash"]);
        assert_eq!(row["status"], 1);
        assert_eq!(
            u64::from_str_radix(
                tx["transactionIndex"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
                16
            )
            .unwrap(),
            row["index"].as_u64().unwrap()
        );
        let ev = sites["logs"]["pmm"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["transactionHash"] == tx["hash"])
            .unwrap();
        assert_eq!(tx["blockHash"], ev["blockHash"]);
        assert_eq!(ev["topics"][2], json!(proof.order_hash));
        let delivered: super::inclusion::Tx = serde_json::from_value(tx.clone()).unwrap();
        let other = tx["from"].as_str().unwrap().parse::<Address>().unwrap() != terms.payer;
        let relay = super::inclusion::Sender::Relay;
        assert_eq!(json!(proof.fill_topics()), ev["topics"]);
        assert_eq!(delivered.matches(proof, relay, keccak256(&data), at), other);
        assert!(!delivered.matches(proof, relay, B256::ZERO, at));
        if other {
            assert_ne!(terms.input, WETH);
            assert!(logs.iter().any(|l| l.topics() == proof.fill_topics()));
            assert!(!delivered.matches(
                proof,
                super::inclusion::Sender::Owner(0),
                keccak256(&data),
                at
            ));
        }
        *makers.entry(proof.maker).or_insert(0) += 1;
        external_sender +=
            usize::from(tx["from"].as_str().unwrap().parse::<Address>().unwrap() != terms.payer);
        if let Some(c) = movements["controls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["hash"] == tx["hash"])
        {
            assert_eq!(c["outer_keccak"], json!(keccak256(&data)));
            assert_eq!(spent, num(&c["before"][0]) - num(&c["after"][0]));
            assert_eq!(received, num(&c["after"][1]) - num(&c["before"][1]));
            controls += 1;
            unchanged += usize::from(c["duplicate"] == "reverted without balance change");
        }
        // Один и тот же proof нельзя зачесть по второй выплате или по одним лишь событиям внешнего пула.
        let mut duplicate = logs.clone();
        duplicate.extend_from_slice(&logs);
        assert!(proof.amounts(&duplicate).is_none());
        assert!(proof.amounts(&[]).is_none());
    }
    let mut counts: Vec<_> = makers.into_values().collect();
    counts.sort_unstable();
    assert_eq!(counts, [29, 72]);
    assert_eq!((controls, unchanged, external_sender), (8, 6, 19));
}
