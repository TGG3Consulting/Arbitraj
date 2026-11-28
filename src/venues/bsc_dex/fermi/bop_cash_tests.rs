//! Реальные кассовые вызовы на двух кадрах Titan и BuilderNet, записанные в GitHub Actions.
use super::*;
use crate::venues::bsc_dex::fermi::{bop_cash::Order, bop_versions, slot};
use flate2::read::GzDecoder;
use serde_json::{Value, json};
use std::str::FromStr;

#[test]
fn published_cash_and_direct_calls_recover_only_the_original_wallet_and_path() {
    let data = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/bopamm_ethereum_cash_wallet.json.gz"
    );
    let archive: Value = serde_json::from_reader(GzDecoder::new(data.as_slice())).unwrap();
    let source = *super::super::bop_binding::Identity::issued().unwrap();
    let market = source.market.address;
    let mut counts = [0, 0];
    for frame in archive["frames"].as_array().unwrap() {
        let get = |v: &Value, key: &str| U256::from_str(v[key].as_str().unwrap()).unwrap();
        for row in frame["cases"].as_array().unwrap() {
            for (idx, name) in ["direct", "compound"].iter().enumerate() {
                let fill_data = &row[*name];
                if fill_data["outcome"] != "paid" {
                    continue;
                }
                counts[idx] += 1;
                let spent = get(fill_data, "spent");
                let net = get(fill_data, "received");
                let input: Address = row["input"].as_str().unwrap().parse().unwrap();
                let buy = input == source.cash.usdt.address;
                let proof = slot::Proof {
                    builder: frame["builder"].as_str().unwrap().into(),
                    parent: frame["parent"].as_str().unwrap().parse().unwrap(),
                    target: frame["height"].as_u64().unwrap(),
                    timestamp: 1_606_824_023 + frame["slot"].as_u64().unwrap() * 12,
                    executor: fill_data["executor"].as_str().unwrap().parse().unwrap(),
                    router: Some(market),
                    prop: None,
                    bop: Some(Box::new(Order {
                        source,
                        bridge: idx == 1,
                        paid: spent,
                        minimum: net,
                        runtime: bop_versions::CASH,
                    })),
                    payer: frame["holder"].as_str().unwrap().parse().unwrap(),
                    input,
                    output: row["output"].as_str().unwrap().parse().unwrap(),
                    amount: if buy {
                        -I256::from_raw(net)
                    } else {
                        I256::from_raw(spent)
                    },
                    limit: if buy { spent } else { net },
                    leg: fill_data["leg"].as_str().unwrap().parse().unwrap(),
                };
                assert_eq!(
                    format!("{:#x}", proof.calldata()),
                    fill_data["data"].as_str().unwrap()
                );
                assert_eq!(fill_data["duplicate_reverted"], true);
                assert_eq!(fill_data["completed"], true);
                assert_eq!(
                    fill_data["duplicate_data"],
                    format!("0x{}", &format!("{:x}", keccak256(b"InvalidOrder()"))[..8])
                );
                let mut receipt:send::receipt::Receipt=serde_json::from_value(json!({"blockNumber":"0x1","status":"0x1","gasUsed":"0x1","effectiveGasPrice":"0x1","logs":[]})).unwrap();
                for log in fill_data["logs"].as_array().unwrap() {
                    receipt.logs.push(alloy::rpc::types::Log {
                        inner: serde_json::from_value(log.clone()).unwrap(),
                        ..Default::default()
                    });
                }
                assert_eq!(fill(&proof, &receipt, market), Some((spent, net)));
                if idx == 1 {
                    equal_maker_payments(
                        &receipt,
                        market,
                        proof.executor,
                        source.cash.usdc.address,
                    );
                }
                let mut wrong = proof.clone();
                wrong.leg = B256::ZERO;
                assert_eq!(fill(&wrong, &receipt, market), None);
                let mut wrong = proof.clone();
                wrong.bop.as_mut().unwrap().source.market.address = Address::repeat_byte(49);
                assert_eq!(fill(&wrong, &receipt, market), None);
                let mut wrong = proof.clone();
                wrong.bop.as_mut().unwrap().source.cash.weth.address = Address::repeat_byte(50);
                assert_eq!(fill(&wrong, &receipt, market), None);
                let mut wrong_logs = fill_data["logs"].clone();
                let change = wrong_logs
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|l| {
                        l["topics"].as_array().is_some_and(|topics| {
                            topics.len() == 3
                                && topics[0]
                                    == format!(
                                        "{:#x}",
                                        keccak256(b"Transfer(address,address,uint256)")
                                    )
                                && topics[1] == format!("{:#x}", market.into_word())
                        })
                    })
                    .unwrap();
                change["topics"][2] = json!(Address::repeat_byte(51).into_word());
                let saved = std::mem::take(&mut receipt.logs);
                receipt.logs = wrong_logs
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| alloy::rpc::types::Log {
                        inner: serde_json::from_value(l.clone()).unwrap(),
                        ..Default::default()
                    })
                    .collect();
                assert_eq!(fill(&proof, &receipt, market), None);
                receipt.logs = saved;
                if idx == 1 {
                    receipt
                        .logs
                        .retain(|l| l.address() != source.cash.pool.address);
                    assert_eq!(fill(&proof, &receipt, market), None);
                }
            }
        }
    }
    assert_eq!(counts, [3, 12]);
}

/// Прежний поиск по всей квитанции считал равные расходы разных maker неоднозначными.
/// Контроль удваивает одну действительно оплаченную долю, меняя обе стороны второго maker.
fn equal_maker_payments(
    receipt: &send::receipt::Receipt,
    market: Address,
    executor: Address,
    usdc: Address,
) {
    use super::super::bop_makers::{MakerFill, Transfer, paid};
    let first = receipt
        .logs
        .iter()
        .position(|l| {
            l.address() == market && l.topics().first() == Some(&MakerFill::SIGNATURE_HASH)
        })
        .unwrap();
    let fragment = &receipt.logs[..=first];
    let f = MakerFill::decode_log_data(&fragment[first].inner.data).unwrap();
    let mut m = receipt
        .logs
        .iter()
        .find_map(|l| {
            (l.address() == executor)
                .then(|| Filled::decode_log_data(&l.inner.data).ok())
                .flatten()
        })
        .unwrap();
    assert_eq!((f.input, f.output), (m.input, m.output));
    let mut second = fragment.to_vec();
    let maker = fragment
        .iter()
        .find_map(|l| {
            if l.address() != f.input {
                return None;
            }
            let t = Transfer::decode_log_data(&l.inner.data).ok()?;
            (t.from == market && t.value == f.spent).then_some(t.to)
        })
        .unwrap()
        .into_word();
    let other = Address::repeat_byte(89).into_word();
    for l in &mut second {
        if l.topics().first() == Some(&Transfer::SIGNATURE_HASH) {
            for topic in l.inner.data.topics_mut().iter_mut().skip(1) {
                if *topic == maker {
                    *topic = other;
                }
            }
        }
    }
    second[first].inner.data.topics_mut()[1] = B256::from(U256::from(f.makerId.wrapping_add(1)));
    let mut repeat = receipt.clone();
    repeat.logs = fragment.iter().cloned().chain(second).collect();
    m.spent = f.spent + f.spent;
    m.received = f.received + f.received;
    assert!(paid(&repeat, market, &m, usdc));
}
