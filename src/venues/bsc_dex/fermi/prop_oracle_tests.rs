//! Записанные кадры обоих строителей и проверка разрешённых ключей цен.
use super::*;
use serde_json::json;
const ARCHIVE: &str = include_str!(
    "../../../../tests/data/first_run_dex_2026-11-26/prop_ethereum_builder_frames.json"
);

#[test]
fn public_prices_keep_only_confirmed_storage_on_the_accepted_oracle() {
    let snapshot: Value = serde_json::from_str(ARCHIVE).unwrap();
    for source in snapshot["frames"].as_array().unwrap() {
        let market = source["market"].as_str().unwrap().parse().unwrap();
        let address: Address = source["oracle"].as_str().unwrap().parse().unwrap();
        let storage = source["storage"].as_object().unwrap();
        let keys = storage.keys().cloned().collect::<Vec<_>>().join(" ");
        let oracle = Oracle::parse(
            market,
            &[format!("{address:#x} {:#x} {keys}", B256::repeat_byte(1))],
        )
        .unwrap();
        let raw = json!({"quoteAddress":market,"blockNumber":snapshot["number"],"slot":snapshot["slot"],
            "stateOverride":{(format!("{address:#x}")):{"nonce":"0x9","balance":"0x123","stateDiff":storage}}});
        let outer = |row: &Value| {
            if source["builder"] == "Titan" {
                json!({"venues":[row]})
            } else {
                json!({(format!("{market:#x}")):row, "blockNumber":format!("0x{:x}",snapshot["number"].as_u64().unwrap()), "slot":snapshot["slot"]})
            }
        };
        let builder = if source["builder"] == "Titan" {
            Builder::Titan
        } else {
            Builder::BuilderNet
        };
        let frame = oracle.frame(builder, &outer(&raw)).unwrap();
        assert_eq!(
            frame.overrides(),
            json!({(format!("{address:#x}")):{"stateDiff":storage}})
        );
        for key in ["state", "code"] {
            let mut changed = raw.clone();
            changed["stateOverride"][format!("{address:#x}")][key] = json!("0xff");
            assert!(oracle.frame(builder, &outer(&changed)).is_none());
        }
        let mut foreign = raw.clone();
        foreign["stateOverride"][format!("{:#x}", Address::repeat_byte(55))] =
            json!({"stateDiff":storage});
        assert!(oracle.frame(builder, &outer(&foreign)).is_none());
        let mut unknown = raw.clone();
        unknown["stateOverride"][format!("{address:#x}")]["stateDiff"]
            [format!("{:#x}", B256::repeat_byte(55))] = json!(B256::ZERO);
        assert!(oracle.frame(builder, &outer(&unknown)).is_none());
    }
}
