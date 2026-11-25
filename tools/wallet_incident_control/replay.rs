fn recorded_cyber() -> Vec<serde_json::Value> {
    serde_json::from_str(include_str!("cyber_four_cases.json")).unwrap()
}

fn verify_card(row: &serde_json::Value) {
    use std::str::FromStr;
    let d = |k: &str| rust_decimal::Decimal::from_str(row[k].as_str().unwrap()).unwrap();
    assert_eq!(d("qty") - d("fee"), d("net"));
    assert_eq!(d("net") - d("withdraw_fee"), d("plan_arrive"));
    assert_eq!(d("gross") - d("invested") - d("costs"), d("pnl"));
    assert_eq!(row["transfers"].as_u64(), Some(0));
}
