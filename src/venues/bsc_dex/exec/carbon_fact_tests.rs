//! Архив контроллера: обе формы ограничения, исходные calldata, фактические ERC20-переводы и повторное чтение.
use super::super::carbon_wire::{Action, Call, ITrade};
use super::{ICarbon, Proof};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, Bytes, U256};
use flate2::read::GzDecoder;
use serde_json::Value;
#[test]
fn original_signed_vector_recovers_each_completed_wallet_trade() {
    let bytes =
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/carbon_ethereum.json.gz");
    let raw: Value = serde_json::from_reader(GzDecoder::new(bytes.as_slice())).unwrap();
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let units = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let wrapped = alloy_primitives::address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
    let mut n = 0;
    for v in raw["controls"].as_array().unwrap() {
        let src = addr(&v["source"]);
        let dst = addr(&v["target"]);
        let normalized = |a| if a == super::NATIVE { wrapped } else { a };
        let t = &v["transaction"];
        let data: Bytes = t["data"].as_str().unwrap().parse().unwrap();
        let by_target = v["by_target"].as_bool().unwrap();
        let (limit, deadline, actions) = if by_target {
            let c = ITrade::tradeByTargetAmountCall::abi_decode_validate(&data).unwrap();
            (c.maxInput, c.deadline, c.tradeActions)
        } else {
            let c = ITrade::tradeBySourceAmountCall::abi_decode_validate(&data).unwrap();
            (c.minReturn, c.deadline, c.tradeActions)
        };
        let call = Call {
            module: None,
            code: None,
            controller: addr(&t["to"]),
            input: normalized(src),
            output: normalized(dst),
            source: src,
            target: dst,
            by_target,
            actions: actions
                .into_iter()
                .map(|a| Action {
                    id: a.strategyId,
                    amount: a.amount,
                })
                .collect(),
            limit,
            deadline,
        };
        let payer = addr(&v["owner"]);
        let input = call.input;
        let output = call.output;
        assert_eq!(call.data(), data);
        assert_eq!(call.value(), units(&t["value"]));
        let saved = serde_json::to_value(Proof { call, payer }).unwrap();
        let mut p: Proof = serde_json::from_value(saved).unwrap();
        let logs: Vec<Log> = serde_json::from_value(v["logs"].clone()).unwrap();
        let amounts = Some((units(&v["input"]), units(&v["output"])));
        assert_eq!(p.amounts(&logs, payer, input, output), amounts);
        assert_eq!(p.amounts(&logs, Address::ZERO, input, output), None);
        assert_eq!(p.amounts(&logs, payer, output, input), None);
        let without_action: Vec<_> = logs
            .iter()
            .filter(|l| l.topic0() != Some(&ICarbon::StrategyUpdated::SIGNATURE_HASH))
            .cloned()
            .collect();
        assert_eq!(p.amounts(&without_action, payer, input, output), None);
        let without_transfer: Vec<_> = logs
            .iter()
            .filter(|l| l.address() == p.call.controller)
            .cloned()
            .collect();
        assert_eq!(p.amounts(&without_transfer, payer, input, output), None);
        p.call.actions[0].id += U256::ONE;
        assert_eq!(p.amounts(&logs, payer, input, output), None);
        n += 1;
    }
    assert_eq!(n, 12);
}
