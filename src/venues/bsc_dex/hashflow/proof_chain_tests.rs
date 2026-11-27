//! Канонические прямые RFQT четырёх сетей и отрицательные формы отмены.
use super::*;
use serde_json::{Value, from_value, json};
#[test]
fn stored_digest_and_old_public_terms_pin_original_but_cancellation_must_be_a_self_transfer() {
    let rows: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../tests/data/hashflow_direct_2026-11-26/swaps.json"
    ))
    .unwrap();
    for row in rows {
        let chain = row["chain_id"].as_u64().unwrap();
        let nonce = row["nonce"].as_u64().unwrap();
        let hash = from_value(row["transaction_hash"].clone()).unwrap();
        let mut data = json!({"hash":hash,"chainId":format!("0x{chain:x}"),"from":row["from"],"to":row["to"],"input":row["input"],
            "nonce":format!("0x{nonce:x}"),"value":row["value"],"blockHash":row["block_hash"],"transactionIndex":row["receipt"]["transactionIndex"]});
        let tx: Transaction = from_value(data.clone()).unwrap();
        let q = wire::tradeRFQTCall::abi_decode_validate(&tx.input)
            .unwrap()
            .quote;
        let public = json!({"pool":q.pool,"trader":q.trader,"input":q.baseToken,"output":q.quoteToken,"txid":q.txid,"amount":q.effectiveBaseTokenAmount,
            "proceeds":q.quoteTokenAmount*q.effectiveBaseTokenAmount/q.baseTokenAmount});
        let mut proof: ReceiptProof = from_value(public).unwrap();
        assert!(proof.original_matches(&tx, chain, hash, nonce));
        assert!(!proof.original_matches(&tx, chain + 1, hash, nonce));
        proof.original = Some(Original {
            chain,
            target: tx.to,
            calldata: keccak256(&tx.input),
        });
        assert!(proof.original_matches(&tx, chain, hash, nonce));
        assert!(!proof.original_matches(&tx, chain, hash, nonce + 1));
        let other = B256::repeat_byte(1);
        data["hash"] = json!(other);
        data["to"] = row["from"].clone();
        data["input"] = json!("0x");
        let cancel: Transaction = from_value(data.clone()).unwrap();
        assert!(cancel.cancelled(chain, other, tx.from, nonce));
        assert!(!cancel.cancelled(chain, hash, tx.from, nonce));
        assert!(!cancel.cancelled(chain + 1, other, tx.from, nonce));
        assert!(!cancel.cancelled(chain, other, tx.from, nonce + 1));
        data["value"] = json!("0x1");
        assert!(
            !from_value::<Transaction>(data.clone())
                .unwrap()
                .cancelled(chain, other, tx.from, nonce)
        );
        data["value"] = json!("0x0");
        data["input"] = json!("0x00");
        assert!(
            !from_value::<Transaction>(data)
                .unwrap()
                .cancelled(chain, other, tx.from, nonce)
        );
    }
}
