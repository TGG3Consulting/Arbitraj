
/// Offline harness installed by CI in this checkout only.
pub fn audit_receipt_credit(
    raw: serde_json::Value, token: Address, wallet: Address, _hash: B256, _expected: Option<alloy_primitives::U256>,
) -> Option<alloy_primitives::U256> {
    let r: Receipt = serde_json::from_value(raw).unwrap();
    let qty = fill::moved(&r, token, None, Some(wallet));
    (!qty.is_zero()).then_some(qty)
}
