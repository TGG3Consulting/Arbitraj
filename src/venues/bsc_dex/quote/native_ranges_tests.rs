use super::*;
use crate::venues::bsc_dex::exec::executor::abi::ArbExecutor;
use alloy::sol_types::SolCall;
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn v5_uses_native_payment_and_reconciles_both_directions_on_three_ranges_pools() {
    let archive: Value = serde_json::from_reader(GzDecoder::new(
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/ranges_ethereum_native.json.gz"
        )
        .as_slice(),
    ))
    .unwrap();
    let addr = |v: &Value| serde_json::from_value::<Address>(v.clone()).unwrap();
    let mut counts = [0usize; 2];
    for row in archive["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["mode"] == "v5")
    {
        let cash = addr(&row["cash"]);
        let token = addr(&row["token"]);
        for (i, (key, input, output)) in [("buy", cash, token), ("sell", token, cash)]
            .into_iter()
            .enumerate()
        {
            let fill = &row[key];
            if fill["stage"] != "paid" {
                continue;
            }
            let amount = fill["debit"].to_string().parse::<U256>().unwrap();
            let received = fill["credit"].to_string().parse::<U256>().unwrap();
            let signed = &fill["signed"];
            let bytes = serde_json::from_value::<Bytes>(signed["data"].clone()).unwrap();
            let decoded = ArbExecutor::executeNativeCall::abi_decode(&bytes).unwrap();
            let [leg] = decoded.steps.as_slice() else {
                panic!("one pool")
            };
            let graph = V5Step {
                native: Some((cash, i == 0)),
                kind: 23,
                pool: leg.pool,
                data: alloy::sol_types::SolValue::abi_encode(&addr(&row["pool"])).into(),
            };
            let order = Order {
                payer: addr(&archive["payer"]),
                target: addr(&archive["executor"]),
                input,
                output,
                amount,
                minimum: received,
                wire: Wire::V5Funded(graph, vec![leg.pool]),
            };
            assert_eq!(calldata(&order, decoded.deadline, received), bytes);
            let value = serde_json::from_value::<U256>(signed["value"].clone()).unwrap();
            assert_eq!(value, if i == 0 { amount } else { U256::ZERO });
            let logs: Vec<crate::venues::bsc_dex::check::simulate::SimLog> =
                serde_json::from_value(fill["logs"].clone()).unwrap();
            let signature = alloy_primitives::keccak256("NativeSettled(uint256,uint256)");
            let settled: Vec<_> = logs
                .iter()
                .filter(|l| l.address == order.target && l.topics.as_slice() == [signature])
                .collect();
            assert_eq!(settled.len(), 1);
            let fields = &settled[0].data;
            assert_eq!(fields.len(), 64);
            assert_eq!(
                (
                    U256::from_be_slice(&fields[..32]),
                    U256::from_be_slice(&fields[32..])
                ),
                if i == 0 {
                    (amount, U256::ZERO)
                } else {
                    (U256::ZERO, received)
                }
            );
            let receipt:crate::venues::bsc_dex::send::track::Receipt=serde_json::from_value(serde_json::json!({
                "status":"0x1","blockNumber":"0x1","gasUsed":"0x1","effectiveGasPrice":"0x0","logs":fill["logs"]
            })).unwrap();
            use crate::core::types::Side;
            use crate::venues::bsc_dex::exec::executor::fact::native_amounts;
            assert_eq!(
                native_amounts(
                    &receipt,
                    order.payer,
                    order.target,
                    (input, output),
                    Side::Buy
                ),
                Some((received, amount))
            );
            assert_eq!(
                native_amounts(
                    &receipt,
                    order.payer,
                    order.target,
                    (input, output),
                    Side::Sell
                ),
                Some((amount, received))
            );
            counts[i] += 1;
        }
    }
    assert_eq!(counts, [27, 25]);
}
