//! Архив контроллера: обе формы ограничения, исходные calldata, фактические ERC20-переводы и повторное чтение.
use super::super::carbon_wire::{Action, Call, ITrade, IWallet};
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
    let read = |bytes: &[u8]| serde_json::from_reader::<_, Value>(GzDecoder::new(bytes)).unwrap();
    let eth = read(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/carbon_ethereum.json.gz"));
    let eth_protected = read(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/carbon_ethereum_protected.json.gz"));
    let base = read(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/alien_carbon_base.json.gz"));
    let eth_wrapped = alloy_primitives::address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let base_wrapped = addr(&base["original"]["roles"]["wrapped"]["address"]);
    let units = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    for (raw, wrapped) in [
        (&eth, eth_wrapped),
        (&eth_protected, eth_wrapped),
        (&base["original"], base_wrapped),
        (&base["protected"], base_wrapped),
    ] {
        let mut n = 0;
        for v in raw["controls"].as_array().unwrap() {
            let src = addr(&v["source"]);
            let dst = addr(&v["target"]);
            let normalized = |a| if a == super::NATIVE { wrapped } else { a };
            let t = &v["transaction"];
            let data: Bytes = t["data"].as_str().unwrap().parse().unwrap();
            let by_target = v["by_target"].as_bool().unwrap();
            let module = raw.get("module").map(addr);
            let (limit, deadline, actions) = if module.is_some() {
                let c = IWallet::executeCall::abi_decode_validate(&data).unwrap();
                assert_eq!((c.source, c.target, c.byTarget), (src, dst, by_target));
                (c.limit, c.deadline, c.actions)
            } else if by_target {
                let c = ITrade::tradeByTargetAmountCall::abi_decode_validate(&data).unwrap();
                (c.maxInput, c.deadline, c.tradeActions)
            } else {
                let c = ITrade::tradeBySourceAmountCall::abi_decode_validate(&data).unwrap();
                (c.minReturn, c.deadline, c.tradeActions)
            };
            let call = Call {
                estimate: None,
                module,
                code: None,
                controller: raw.get("controller").map_or_else(|| addr(&t["to"]), addr),
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
            assert_eq!(call.recipient(), addr(&t["to"]));
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
            if let Some(module) = p.call.module {
                let without_receipt: Vec<_> = logs
                    .iter()
                    .filter(|l| l.address() != module)
                    .cloned()
                    .collect();
                assert_eq!(p.amounts(&without_receipt, payer, input, output), None);
                if p.call.by_target && src == super::NATIVE {
                    assert_eq!(p.call.value(), units(&v["input"]) + U256::ONE);
                }
            }
            p.call.actions[0].id += U256::ONE;
            assert_eq!(p.amounts(&logs, payer, input, output), None);
            n += 1;
        }
        assert_eq!(n, 12);
    }
}
