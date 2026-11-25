
/// Offline harness installed by CI in this checkout only.
pub fn audit_receipt_credit(
    raw: serde_json::Value, token: Address, wallet: Address, hash: B256, expected: Option<alloy_primitives::U256>,
) -> Option<alloy_primitives::U256> {
    let r: Receipt = serde_json::from_value(raw).unwrap();
    let logs = super::deposit_log::incoming(&r, token, wallet, hash).ok()?;
    super::deposit_log::select(&logs, expected).ok().map(|(_, qty)| qty)
}
