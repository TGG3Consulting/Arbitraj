use arb::gateway::cpu_budget::{Limits, Tier, acquire, try_optional};

#[tokio::test]
async fn one_slot_remains_available_to_both_mandatory_stages() {
    let one = Limits {
        total: 1,
        reserve_deal: 1,
        reserve_calc: 0,
    };
    let first = acquire(one, Tier::Deal).await;
    assert!(try_optional(Some(one)).is_none());
    drop(first);
    assert!(try_optional(Some(one)).is_none());
    let second = acquire(one, Tier::Deal).await;
    drop(second);
    assert!(try_optional(None).is_none());
    let no_reserve = Limits {
        total: 2,
        reserve_deal: 0,
        reserve_calc: 0,
    };
    assert!(try_optional(Some(no_reserve)).is_none());
    let two = Limits {
        total: 2,
        reserve_deal: 1,
        reserve_calc: 0,
    };
    let spare = try_optional(Some(two)).unwrap();
    assert!(try_optional(Some(two)).is_none());
    let required = acquire(two, Tier::Deal).await;
    drop((spare, required));
}
