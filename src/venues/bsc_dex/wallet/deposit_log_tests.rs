use super::*;
use crate::storage::db::Db;
use crate::venues::bsc_dex::num;
use rust_decimal::Decimal;

fn cases() -> [(Receipt, B256, Address, Address); 2] {
    [
        (
            include_str!("../../../../tests/data/batched_deposit_2026_11_24/tlos.json"),
            "0x55d398326f99059ff775485246999027b3197955",
        ),
        (
            include_str!("../../../../tests/data/batched_deposit_2026_11_24/num.json"),
            "0xeceb87cf00dcbf2d4e2880223743ff087a995ad9",
        ),
    ]
    .map(|(s, token)| {
        let raw: serde_json::Value = serde_json::from_str(s).unwrap();
        let hash = raw["transactionHash"].as_str().unwrap().parse().unwrap();
        let wallet = "0xdfcbe6dd5056c896066db400f414e2c353c095e9"
            .parse()
            .unwrap();
        (
            serde_json::from_value(raw).unwrap(),
            hash,
            token.parse().unwrap(),
            wallet,
        )
    })
}

#[test]
fn tlos_and_num_match_individual_logs_and_refuse_ambiguous_credit() {
    for ((mut r, h, token, w), amounts) in cases().into_iter().zip([
        [(143, "61.35492211"), (144, "122.59313556")],
        [(70, "5973.27"), (72, "19366.1271")],
    ]) {
        assert!(pick(&r, token, w, h, None).is_err());
        for (index, amount) in amounts {
            let raw = num::to_units(amount.parse().unwrap(), 18).unwrap();
            assert_eq!(pick(&r, token, w, h, Some(raw)), Ok((index, raw)));
        }
        let chosen = r
            .logs
            .iter()
            .find(|l| l.log_index == Some(amounts[0].0))
            .unwrap()
            .clone();
        r.logs = vec![chosen.clone()];
        assert_eq!(pick(&r, token, w, h, None).unwrap().0, amounts[0].0);
        // A single payment belonging to the second withdrawal must remain unclaimed
        // when the first has a different, independently confirmed net.
        let other = num::to_units(amounts[1].1.parse().unwrap(), 18).unwrap();
        assert!(pick(&r, token, w, h, Some(other)).is_err());
        let mut equal = chosen;
        equal.log_index = Some(amounts[1].0);
        r.logs.push(equal);
        assert!(
            pick(
                &r,
                token,
                w,
                h,
                Some(num::to_units(amounts[0].1.parse().unwrap(), 18).unwrap())
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn reservation_survives_restart_and_cannot_switch_withdrawal_or_log() {
    let tmp = std::env::temp_dir().join(format!(
        "arb-log-claim-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_micros()
    ));
    std::fs::create_dir(&tmp).unwrap();
    let path = tmp.join("arb.sqlite");
    let claim = |id: &str| DepositClaim {
        sender: crate::core::types::VenueId("kucoin".into()),
        withdrawal_id: id.into(),
        deal: format!("deal-{id}"),
        expected: None,
    };
    let key = ["56", "usdt", "w4", "tx"].map(str::to_string);
    let check = |db: Db, index, who: DepositClaim, amount: &str| {
        let k = key.clone();
        let raw = amount.to_owned();
        async move {
            db.ledger()
                .write(move |conn| reserve(conn, &k, index, &raw, &who))
                .await
                .unwrap()
        }
    };
    let db = Db::open(&path).unwrap();
    assert!(check(db.clone(), 143, claim("A"), "61").await);
    drop(db);
    let db = Db::open(&path).unwrap();
    let mut same = claim("A");
    same.expected = Some(Decimal::ONE);
    let saved_key = key.clone();
    let saved_owner = same.clone();
    assert_eq!(
        db.ledger()
            .read(move |conn| find(conn, &saved_key, &saved_owner))
            .await
            .unwrap(),
        Some((143, "61".into()))
    );
    assert!(check(db.clone(), 143, same, "61").await);
    assert!(!check(db.clone(), 143, claim("B"), "61").await);
    assert!(!check(db.clone(), 144, claim("A"), "122").await);
    assert!(!check(db.clone(), 143, claim("A"), "62").await);
    assert!(check(db, 144, claim("B"), "122").await);
    std::fs::remove_dir_all(tmp).unwrap();
}

fn pick(
    r: &Receipt,
    t: Address,
    w: Address,
    h: B256,
    expected: Option<U256>,
) -> Result<(u64, U256), &'static str> {
    select(&incoming(r, t, w, h)?, expected)
}

#[tokio::test]
async fn websocket_reuses_only_its_confirmed_chain_wallet_and_transaction() {
    use super::super::inflows::Inflows;
    use std::sync::Arc;
    let [(receipt, hash, token, wallet), _] = cases();
    let inflows = Inflows::default();
    let proof = Arc::new(receipt);
    let policy = (3, std::time::Duration::from_secs(600));
    inflows.expect(token).await;
    assert!(inflows.confirmed(token, 56, wallet, hash, policy).is_none());
    inflows.remember(token, 56, wallet, hash, proof.clone(), 3);
    assert!(Arc::ptr_eq(
        &proof,
        &inflows.confirmed(token, 56, wallet, hash, policy).unwrap()
    ));
    assert!(
        inflows
            .confirmed(token, 42161, wallet, hash, policy)
            .is_none()
    );
    assert!(
        inflows
            .confirmed(token, 56, Address::ZERO, hash, policy)
            .is_none()
    );
    assert!(
        inflows
            .confirmed(token, 56, wallet, B256::ZERO, policy)
            .is_none()
    );
    assert!(
        inflows
            .confirmed(token, 56, wallet, hash, (4, policy.1))
            .is_none()
    );
    inflows.expect(token).await;
    inflows.done(token, "first");
    inflows.done(token, "first");
    assert!(inflows.confirmed(token, 56, wallet, hash, policy).is_some());
    inflows.abandoned(token);
    assert_eq!(Arc::strong_count(&proof), 1);
    inflows.expect(token).await;
    assert!(inflows.confirmed(token, 56, wallet, hash, policy).is_none());
    inflows.remember(token, 56, wallet, hash, proof.clone(), 3);
    assert!(
        inflows
            .confirmed(token, 56, wallet, hash, (3, std::time::Duration::ZERO))
            .is_none()
    );
    assert_eq!(Arc::strong_count(&proof), 1);
}
