#[tokio::test]
async fn cyber_four_refusals() {
    let path = dir("cyber_recorded_four");
    let (core, _) = start(&path, None).await;
    let ctx = bn(&core, &path);
    set(&core, "venue.withdraw_unready_window_ms", json!(16000)).await;
    for row in recorded_cyber() {
        verify_card(&row);
        let elapsed = row["elapsed_ms"].as_u64().unwrap();
        withdraw_ready::traded("CYBER", tokio::time::Instant::now());
        tokio::time::sleep(std::time::Duration::from_millis(elapsed)).await;
        let error = refusal(row["code"].as_i64().unwrap(), row["msg"].as_str().unwrap());
        let e = transfers::not_ready(&ctx, "CYBER", error);
        assert_eq!(e.code, ErrorCode::WithdrawLocked);
        println!("CYBER_RESULT {} {}ms {:?}", row["id"], elapsed, e.code);
    }
}
