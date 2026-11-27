//! Рекомендации на основании записанного рабочего снимка и квитанции Dexalot Base.
use rust_decimal::Decimal;
use serde_json::Value;
use std::str::FromStr;

#[test]
fn recorded_global_settings_and_base_receipt_support_operating_advice() {
    let evidence: Value = serde_json::from_str(include_str!(
        "../../tests/data/first_run_dex_2026-11-26/first_run_operating_evidence.json"
    )).unwrap();
    let shipped: toml::Value = toml::from_str(include_str!("../../config/first_run.toml")).unwrap();
    let rows: Vec<_> = shipped["section"].as_array().unwrap().iter()
        .flat_map(|section| section.get("rec").and_then(toml::Value::as_array).into_iter().flatten())
        .collect();
    for (key, observed) in evidence["settings"].as_object().unwrap() {
        let row = rows.iter().find(|row| row["name"].as_str() == Some(key.as_str())
            && row.get("prefill").and_then(|p| p.get("kind")).and_then(toml::Value::as_str) == Some("Global"))
            .unwrap_or_else(|| panic!("нет глобальной рекомендации {key}"));
        let value = observed.as_str().map(str::to_owned).unwrap_or_else(|| observed.to_string());
        assert_eq!(row["value"].as_str(), Some(value.as_str()), "{key}");
    }
    let control = &evidence["dexalot_base"];
    let receipt = &control["receipt"];
    let hex = |field: &str| {
        let text = receipt[field].as_str().unwrap().strip_prefix("0x").unwrap();
        u128::from_str_radix(text, 16).unwrap()
    };
    let wei = hex("gasUsed") * hex("effectiveGasPrice") + hex("l1Fee");
    assert_eq!(wei.to_string(), control["fee_wei"].as_str().unwrap());
    let budget = rows.iter().find(|r| r["name"].as_str() == Some("dexalot.gas_budget_native")
        && r["prefill"]["id"].as_str() == Some("base_dex")).unwrap();
    let margin = evidence["settings"]["kyber.gas_limit_margin_pct"].as_str().unwrap().parse::<u128>().unwrap();
    // Резерв до следующего 10^-8 ETH, как указано в карточке первого запуска.
    let units = (wei * (100 + margin)).div_ceil(100 * 10_000_000_000);
    assert_eq!(Decimal::from_str(budget["value"].as_str().unwrap()).unwrap(), Decimal::from_i128_with_scale(i128::try_from(units).unwrap(), 8));
}
