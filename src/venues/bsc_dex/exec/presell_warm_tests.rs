use super::*;
use crate::core::types::{Asset, ChainId};
use alloy_primitives::{address, b256};

#[test]
fn market_subscription_does_not_schedule_checks_and_two_interests_share_one_job() {
    let tokens = super::super::super::tokens::Tokens::default();
    let subscribed = (1..=1000u64)
        .map(|n| Asset {
            ticker: format!("T{n}"),
            chain: Some(ChainId("bsc".into())),
            contract: Some(Address::from_word(B256::from(
                U256::from(n).to_be_bytes::<32>(),
            ))),
        })
        .collect::<Vec<_>>();
    let cache = Cache::default();
    tokens.set(&subscribed);
    let token = subscribed[5].contract.unwrap();
    let accounts = [
        address!("0000000000000000000000000000000000000011"),
        address!("0000000000000000000000000000000000000012"),
    ];
    let mut s = cache.state.lock().unwrap();
    assert_eq!(tokens.all().len(), 1000);
    assert!(s.jobs.is_empty() && s.deployments.is_empty());
    let key = (accounts[0], token, 1);
    let Admission::New(id, _) = s.admit(key) else {
        panic!("first candidate")
    };
    assert!(matches!(s.admit(key),Admission::Existing(same) if id==same));
    assert_eq!((s.starts, s.joins, s.jobs.len()), (1, 1, 1));
    s.release(key, id);
    assert!(s.jobs.contains_key(&key));
    s.release(key, id);
    assert!(s.jobs.is_empty());
}

#[test]
fn late_reorg_or_other_holder_cannot_replace_or_reject_current_wallet_proof() {
    let cache = Cache::default();
    let w = Address::repeat_byte(1);
    let t = Address::repeat_byte(2);
    let key = (w, t, 3);
    let h = B256::repeat_byte(4);
    let h2 = B256::repeat_byte(5);
    let Admission::New(job, _) = cache.state.lock().unwrap().admit(key) else {
        panic!()
    };
    let stamp = Stamp {
        job,
        generation: 3,
        number: 7,
        hash: h,
    };
    let id = layout::Identity::test_direct(b256!(
        "97a48aa4c129657440dafdacd4c836389734d28cc4a0ca7403e68da660a74a59"
    ));
    let funds = Ok((
        id,
        layout::BalanceField {
            storage: t,
            key: B256::repeat_byte(8),
            max: U256::MAX,
        },
        U256::from(5),
    ));
    cache.publish(key, stamp, funds.clone(), None);
    assert!(cache.read(w, t, 8, 3).is_err());
    assert!(cache.read(t, w, 7, 3).is_err());
    assert!(cache.read(w, t, 7, 4).is_err());
    let newer = Stamp { hash: h2, ..stamp };
    cache.publish(key, newer, funds.clone(), None);
    cache.reject(w, t, stamp, "old hash");
    assert_eq!(cache.read(w, t, 7, 3).unwrap().0.hash, h2);
    let bound = cache.binding(t, 3, h2);
    assert!(Arc::ptr_eq(&bound, &cache.binding(t, 3, h2)));
    assert!(!Arc::ptr_eq(&bound, &cache.binding(t, 3, h)));
    cache.reject(w, t, newer, "balanceOf");
    assert!(cache.read(w, t, 7, 3).is_err());
    let mut s = cache.state.lock().unwrap();
    s.release(key, job);
    let Admission::New(next, _) = s.admit(key) else {
        panic!()
    };
    drop(s);
    cache.publish(key, stamp, funds, None);
    assert_ne!(next, job);
    assert!(cache.read(w, t, 7, 3).is_err());
}

#[test]
fn active_signals_above_idle_limit_are_all_admitted_and_cleaned_on_cancel() {
    let mut s = State::default();
    let mut live = Vec::new();
    for i in 1..=123u64 {
        let t = Address::from_word(B256::from(U256::from(i).to_be_bytes::<32>()));
        let key = (Address::repeat_byte(7), t, 1);
        let Admission::New(id, _) = s.admit(key) else {
            panic!("unique candidate")
        };
        live.push((key, id));
    }
    assert_eq!(s.jobs.len(), live.len());
    for (key, id) in live {
        s.release(key, id);
    }
    assert!(s.jobs.is_empty() && s.recent.is_empty() && s.deployments.is_empty());
}
