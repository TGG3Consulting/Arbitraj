//! Контроль подписанной calldata и оплаченных сумм на четырёх реально проведённых прямых обменах.
use super::{amount, router, sim, units, wire};
use crate::gateway::hashflow::Request;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::Deserialize;

#[derive(Deserialize)]
struct Row {
    chain_id: u64,
    to: Address,
    from: Address,
    input: Bytes,
    value: U256,
    status: U256,
    block_timestamp: u64,
    trade_logs: Vec<Log>,
    receipt: super::super::send::receipt::Receipt,
}
#[derive(Deserialize)]
struct Log {
    address: Address,
    data: Bytes,
    topics: Vec<B256>,
}

/// Повтор декодирования по первичным транзакциям (без доступа к закрытому RFQ API).
#[test]
fn mined_swaps_match_the_same_router_abi_and_amounts() {
    use alloy::sol_types::SolEvent;
    let rows: Vec<Row> = serde_json::from_str(include_str!(
        "../../../../tests/data/hashflow_direct_2026-11-26/swaps.json"
    ))
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.chain_id).collect::<Vec<_>>(),
        [1, 56, 8453, 42161]
    );
    for r in rows {
        assert_eq!(r.to, router(r.chain_id).unwrap());
        assert_eq!(r.status, U256::from(1u8));
        let call = wire::tradeRFQTCall::abi_decode_validate(&r.input).unwrap();
        assert_eq!(call.abi_encode().as_slice(), r.input.as_ref());
        let q = call.quote;
        assert_eq!(q.trader, r.from);
        assert_eq!(q.effectiveTrader, r.from);
        assert!(q.quoteExpiry > U256::from(r.block_timestamp));
        assert_eq!(
            r.value,
            if q.baseToken.is_zero() {
                q.effectiveBaseTokenAmount
            } else {
                U256::ZERO
            }
        );
        let req = Request {
            trader: r.from,
            input: q.baseToken,
            output: q.quoteToken,
            amount: Some(q.effectiveBaseTokenAmount.to_string()),
            amount_tokens: None,
        };
        let output = q.effectiveBaseTokenAmount * q.quoteTokenAmount / q.baseTokenAmount;
        let [log] = r.trade_logs.as_slice() else {
            panic!("ожидалось одно подтверждённое событие Trade")
        };
        assert_eq!(log.address, q.pool);
        assert_eq!(log.topics, [wire::Trade::SIGNATURE_HASH]);
        let proof:super::ReceiptProof=serde_json::from_value(serde_json::json!({"pool":q.pool,"trader":r.from,"input":q.baseToken,"output":q.quoteToken,"txid":q.txid,"amount":q.effectiveBaseTokenAmount,"proceeds":output})).unwrap();
        assert!(proof.matches(&r.receipt));
        let mut missing = r.receipt.clone();
        missing.logs.retain(|l| l.address() != q.baseToken);
        assert!(!proof.matches(&missing));
        assert!(sim::event_matches(
            &log.data,
            &req,
            q.txid,
            q.effectiveBaseTokenAmount,
            output
        ));
        assert!(!sim::event_matches(
            &log.data,
            &req,
            q.txid,
            q.effectiveBaseTokenAmount,
            output + U256::from(1u8)
        ));
        // Строка raw и точное отображение одного и того же зафиксированного числа в минимальных единицах.
        let dp = 18;
        let decimal = units(q.effectiveBaseTokenAmount, dp);
        let tokens = Request {
            amount: None,
            amount_tokens: Some(decimal.clone()),
            ..req
        };
        assert_eq!(amount(&tokens, dp).unwrap().0, q.effectiveBaseTokenAmount);
        let too_small = units(
            q.effectiveBaseTokenAmount * U256::from(10u8) + U256::from(1u8),
            dp + 1,
        );
        let fractional = Request {
            amount_tokens: Some(too_small),
            ..tokens
        };
        assert!(amount(&fractional, dp).is_err());
    }
}
