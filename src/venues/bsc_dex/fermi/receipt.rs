//! Сверка событий исполнителя, роутера и чистого движения двух токенов у плательщика. Вызывается при восстановлении квитанции.
use super::super::{exec, send};
use super::slot::{Filled, Proof, Swapped};
use alloy::{primitives::U256, sol_types::SolEvent};
pub(super) fn fill(
    proof: &Proof,
    receipt: &send::receipt::Receipt,
    router: alloy::primitives::Address,
) -> Option<(U256, U256)> {
    if proof.bop.is_some() { return super::bop_receipt::fill(proof,receipt,router); }
    let one = |who, topic| {
        let mut logs = receipt
            .logs
            .iter()
            .filter(|l| l.address() == who && l.topics().first() == Some(&topic));
        let first = logs.next()?;
        if logs.next().is_some() {
            None
        } else {
            Some(first)
        }
    };
    if let Some(q) = &proof.prop {
        use super::slot::SlotFilled;
        if router != q.terms.router {
            return None;
        }
        let mark = SlotFilled::decode_log_data(
            &one(proof.executor, SlotFilled::SIGNATURE_HASH)?.inner.data,
        )
        .ok()?;
        let (source, spent, received) = q.outcome(&receipt.logs)?;
        return ((mark.leg, mark.payer, mark.venue, mark.spent, mark.received)
            == (proof.leg, proof.payer, q.terms.venue, spent, received)
            && source == q.terms.venue
            && proof.crossed(spent, received))
        .then_some((spent, received));
    }
    let f =
        Filled::decode_log_data(&one(proof.executor, Filled::SIGNATURE_HASH)?.inner.data).ok()?;
    let s = Swapped::decode_log_data(&one(router, Swapped::SIGNATURE_HASH)?.inner.data).ok()?;
    if (f.payer, f.leg, f.input, f.output) != (proof.payer, proof.leg, proof.input, proof.output)
        || (s.sender, s.tokenIn, s.tokenOut, s.recipient)
            != (proof.executor, proof.input, proof.output, proof.payer)
        || (f.spent, f.received) != (s.amountIn, s.amountOut)
        || !proof.crossed(f.spent, f.received)
    {
        return None;
    }
    let moved = |token, send| {
        if send {
            exec::fill::moved(receipt, token, Some(proof.payer), None)
        } else {
            exec::fill::moved(receipt, token, None, Some(proof.payer))
        }
    };
    let spent = moved(proof.input, true).checked_sub(moved(proof.input, false))?;
    let received = moved(proof.output, false).checked_sub(moved(proof.output, true))?;
    ((spent, received) == (f.spent, f.received)).then_some((spent, received))
}
#[cfg(test)]
mod tests {
    use super::super::tests::{deployment, recommendation};
    use super::*;
    use alloy::primitives::B256;
    use alloy::primitives::I256;
    use serde_json::Value;
    use std::str::FromStr;

    #[test]
    fn both_builders_confirm_payer_debit_after_refund_and_isolate_other_legs() {
        let f: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/fermi_slot_receipts.json"
        ))
        .unwrap();
        assert_eq!(
            f["runtime_keccak"],
            recommendation("fermi.executor_runtime_hash")
        );
        let router = deployment().router;
        let cases = f["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 8);
        for c in cases {
            let strval = |k: &str| c[k].as_str().unwrap();
            let proof = Proof {
                bop: None,
                prop: None,
                builder: strval("builder").into(),
                parent: strval("parent").parse().unwrap(),
                target: c["target"].as_u64().unwrap(),
                timestamp: c["timestamp"].as_u64().unwrap(),
                executor: f["executor"].as_str().unwrap().parse().unwrap(),
                router: Some(router),
                payer: strval("payer").parse().unwrap(),
                input: strval("input").parse().unwrap(),
                output: strval("output").parse().unwrap(),
                amount: I256::from_str(strval("amount_specified")).unwrap(),
                limit: U256::from_str_radix(strval("limit"), 10).unwrap(),
                leg: strval("leg").parse().unwrap(),
            };
            // eth_simulateV1 returns event payloads without mined-log metadata.
            // Parse the archived payload first, then attach its recorded enclosing block.
            let mut raw = c["receipt"].clone();
            let events = std::mem::replace(&mut raw["logs"], serde_json::json!([]));
            let mut r: send::receipt::Receipt = serde_json::from_value(raw).unwrap();
            for event in events.as_array().unwrap() {
                let inner = serde_json::from_value(event.clone()).unwrap();
                r.logs.push(alloy::rpc::types::Log {
                    inner,
                    block_number: Some(r.block_number.to()),
                    ..Default::default()
                });
            }
            assert_eq!(r.gas_used.to::<u64>(), c["swap_gas"].as_u64().unwrap());
            let spent = U256::from_str_radix(strval("spent"), 10).unwrap();
            let received = U256::from_str_radix(strval("received"), 10).unwrap();
            assert_eq!(fill(&proof, &r, router), Some((spent, received)));
            assert!(fill(&proof, &r, proof.executor).is_none());
            let mut legacy = serde_json::to_value(&proof).unwrap();
            legacy.as_object_mut().unwrap().remove("router");
            let legacy: Proof = serde_json::from_value(legacy).unwrap();
            assert!(legacy.router.is_none());
            assert_eq!(fill(&legacy, &r, router), Some((spent, received)));
            let refund = U256::from_str_radix(strval("refunded"), 10).unwrap();
            assert_eq!(
                exec::fill::moved(&r, proof.input, Some(proof.payer), None),
                spent + refund
            );
            assert_eq!(proof.amount.is_negative(), !refund.is_zero());
            let mut wrong = proof.clone();
            wrong.leg = B256::ZERO;
            assert!(fill(&wrong, &r, router).is_none());
            wrong = proof.clone();
            wrong.payer = proof.executor;
            assert!(fill(&wrong, &r, router).is_none());
            let mut duplicate = r.clone();
            duplicate.logs.extend(r.logs.clone());
            assert!(fill(&proof, &duplicate, router).is_none());
        }
    }
}

#[cfg(test)]
#[path = "prop_receipt_tests.rs"]
mod prop_receipt_tests;

#[cfg(test)]
#[path = "prop_fact_tests.rs"]
mod prop_fact_tests;
