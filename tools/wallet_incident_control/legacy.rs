#[test]
fn cyber_four_refusals() {
    use arb::venues::binance::errors::{from_code, Op};
    use arb::gateway::errors::ErrorCode;
    for row in recorded_cyber() {
        verify_card(&row);
        let code = row["code"].as_i64().unwrap();
        let msg = row["msg"].as_str().unwrap();
        let raw = serde_json::json!({"code":code,"msg":msg}).to_string();
        let e = from_code(code, msg, Op::Withdraw, &raw);
        assert_eq!(e.code, ErrorCode::WithdrawRejected);
        println!("CYBER_RESULT {} {}ms {:?}", row["id"], row["elapsed_ms"], e.code);
    }
}
