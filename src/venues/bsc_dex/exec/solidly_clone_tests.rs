//! Проверка исходного ABI, балансов и состояния после первой ноги на четырёх стабильных и восьми смешанных рынках.
use super::solidly_router_wallet::{Market, Proof};
use crate::venues::bsc_dex::math::{solidly_math::fee_denominator, swap_solidly};
use alloy::rpc::types::Log;
use alloy_primitives::{Address, Bytes, U256, keccak256};
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn both_classic_factories_match_initial_quotes_each_round_trip_and_restored_receipts() {
    let dir: Value = serde_json::from_slice(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/classic_clone_base_tokens.json"
    ))
    .unwrap();
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let uint = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    for (raw, count) in [
        (
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/virtus_amm_base_wallets.json.gz"
            )
            .as_slice(),
            16,
        ),
        (
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/hydrometer_base_wallets.json.gz"
            )
            .as_slice(),
            28,
        ),
    ] {
        let book: Value = serde_json::from_reader(GzDecoder::new(raw)).unwrap();
        assert_eq!(book["selector"], dir["selector"]);
        let rows: Vec<_> = book["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["wire"] == "router")
            .collect();
        assert_eq!(rows.len(), count);
        for legs in rows.chunks_exact(2) {
            let m = book["markets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["pool"] == legs[0]["pool"])
                .unwrap();
            let tokens = [addr(&m["tokens"][0]), addr(&m["tokens"][1])];
            let scales = tokens.map(|a| {
                U256::from(10).pow(U256::from(
                    dir["tokens"][format!("{a:#x}")]["units"].as_u64().unwrap(),
                ))
            });
            let mut reserves = [uint(&m["reserves"][0]), uint(&m["reserves"][1])];
            for r in legs {
                assert_eq!(r["pool"], m["pool"]);
                let i = usize::from(tokens[0] != addr(&r["input"]));
                let j = 1 - i;
                let fee = U256::from(m["fee"].as_u64().unwrap());
                let stable = m["stable"].as_bool().unwrap();
                let quote = |amount: U256| {
                    let net = amount.checked_sub(amount.checked_mul(fee)? / fee_denominator())?;
                    if stable {
                        swap_solidly::stable_net(
                            net,
                            [reserves[i], reserves[j]],
                            [scales[i], scales[j]],
                            Some(scales),
                        )
                    } else {
                        net.checked_mul(reserves[j])?
                            .checked_div(reserves[i].checked_add(net)?)
                    }
                };
                let paid = uint(&r["spent"]);
                let out = uint(&r["received"]);
                assert_eq!(quote(paid), Some(uint(&r["quoted"])));
                let need = swap_solidly::input_for(paid, out, quote).unwrap();
                assert!(need <= paid && quote(need).unwrap() >= out);
                if need > U256::ONE {
                    assert!(quote(need - U256::ONE).is_none_or(|previous| previous < out));
                }
                let call: Bytes = r["tx"]["data"].as_str().unwrap().parse().unwrap();
                let p = Proof {
                    market: Market {
                        pool: addr(&m["pool"]),
                        stable: Some(stable),
                        supporting_fee: false,
                        factory: Some(addr(&book["roles"]["factory"]["address"])),
                    },
                    target: addr(&r["tx"]["to"]),
                    payer: addr(&r["tx"]["from"]),
                    input: tokens[i],
                    output: tokens[j],
                    budget: paid,
                    minimum: out,
                    deadline: U256::from_be_slice(&call[4 + 4 * 32..4 + 5 * 32]),
                };
                let back: Proof = serde_json::from_value(serde_json::to_value(p).unwrap()).unwrap();
                assert_eq!(back.data(), call);
                let logs: Vec<Log> = serde_json::from_value(r["logs"].clone()).unwrap();
                assert_eq!(
                    back.amounts(&logs, back.payer, back.input, back.output),
                    Some((paid, out))
                );
                let sync = logs
                    .iter()
                    .find(|log| {
                        log.address() == back.market.pool
                            && log.topics().first() == Some(&keccak256("Sync(uint256,uint256)"))
                    })
                    .unwrap();
                reserves = [
                    U256::from_be_slice(&sync.data().data[..32]),
                    U256::from_be_slice(&sync.data().data[32..]),
                ];
            }
        }
    }
}
