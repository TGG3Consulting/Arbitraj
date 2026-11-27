//! Восстановление проверяет сохранённые ключи кошелька и позицию исходной квитанции.
use super::*;
use alloy::primitives::{Address, B256};
#[test]
fn recovery_reuses_only_the_original_mixed_wallet_and_leg_even_outside_an_active_scope() {
    let leg = alloy_primitives::keccak256("deal-a/coin-buy");
    assert!(outer_matches(
        "w2",
        Some("w2"),
        "deal-a/coin-buy",
        None,
        leg
    ));
    assert!(outer_matches(
        "w2",
        Some("w2"),
        "deal-a/coin-buy",
        Some("deal-a/coin-buy"),
        leg
    ));
    assert!(!outer_matches(
        "w2",
        Some("w3"),
        "deal-a/coin-buy",
        None,
        leg
    ));
    assert!(!outer_matches(
        "w2",
        Some("w2"),
        "deal-a/coin-buy",
        Some("deal-b/coin-buy"),
        leg
    ));
    assert!(!outer_matches(
        "w2",
        Some("w2"),
        "deal-a/cash-sell",
        None,
        leg
    ));
}

#[test]
fn rpc_inclusion_preserves_identity_for_canonical_position_checks() {
    let tx = B256::with_last_byte(1);
    let block = B256::with_last_byte(2);
    let raw = json!({"from":Address::ZERO,"to":Address::ZERO,"transactionHash":tx,"blockHash":block,"transactionIndex":"0x3",
        "blockNumber":"0x5","gasUsed":"0x5208","effectiveGasPrice":"0x1","status":"0x1","logs":[]});
    let r: Inclusion = serde_json::from_value(raw).unwrap();
    assert_eq!(
        (
            r.receipt.transaction_hash,
            r.receipt.block_hash,
            r.receipt.transaction_index
        ),
        (Some(tx), Some(block), Some(U64::from(3)))
    );
}
