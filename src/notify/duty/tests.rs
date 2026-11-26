//! Регрессии единиц, независимых порогов, повтора и восстановления очереди в SQLite.
use super::*;
use crate::core::deal::Step;
use crate::core::types::{Asset, Direction, VenueId};
use rust_decimal::Decimal;
use serde_json::json;

fn deal() -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "GAIB".into(),
            chain: None,
            contract: None,
        },
        buy_on: VenueId("mexc".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut d = Deal::new("t1GAIB".into(), dir, json!(null), Decimal::from(99));
    d.invested = Decimal::from(3);
    d
}

fn alert(d: &Deal, pnl: Option<Decimal>) -> StuckAlert {
    let net = pnl.map(|p| p + d.invested);
    StuckAlert {
        id: "alert-1".into(),
        deal_id: d.id.clone(),
        venue: d.at.clone(),
        asset: d.direction.asset.clone(),
        qty: Decimal::from(7000),
        loss_pct: net.map(|v| d.loss_pct_if(v)),
        loss_usdt: net.map(|v| -d.result_if(v)),
        reason: None,
        at: d.updated_at,
        error: None,
        attempts: 1,
        holders: Vec::new(),
    }
}

#[test]
fn thresholds_use_incident_pnl_inclusive_or_and_order_on_unknown_price() {
    let d = deal();
    let a = alert(&d, Some(-Decimal::ONE));
    let i = Incident::alert(&a, &d);
    let usd = Thresholds {
        usd: Some(Decimal::ONE),
        ..Default::default()
    };
    assert!(usd.qualifies(&i));
    let percentage = Thresholds {
        pct: a.loss_pct,
        usd: Some(Decimal::from(5)),
        ..Default::default()
    };
    assert!(percentage.qualifies(&i));
    let old: StuckAlert = {
        let mut value = serde_json::to_value(&a).unwrap();
        value.as_object_mut().unwrap().remove("loss_usdt");
        serde_json::from_value(value).unwrap()
    };
    assert!(usd.qualifies(&Incident::alert(&old, &d))); // ровно 1 USDT из 3
    let just_above = Thresholds {
        usd: Some(Decimal::new(10001, 4)),
        ..Default::default()
    };
    assert!(!just_above.qualifies(&i));
    let zero_loss = Thresholds {
        pct: Some(Decimal::ZERO),
        usd: Some(Decimal::ZERO),
        order: None,
    };
    for pnl in [None, Some(Decimal::ZERO), Some(Decimal::ONE)] {
        let i = Incident::alert(&alert(&d, pnl), &d);
        assert!(!zero_loss.qualifies(&i));
        assert!(
            Thresholds {
                order: Some(d.invested),
                ..Default::default()
            }
            .qualifies(&i)
        );
        assert!(
            !Thresholds {
                order: Some(Decimal::from(4)),
                ..Default::default()
            }
            .qualifies(&i)
        );
    }
}

#[test]
fn repeat_first_crosses_threshold_and_later_snapshot_keeps_first_loss() {
    let d = deal();
    let mut pending = BTreeMap::new();
    let limits = Thresholds {
        usd: Some(Decimal::ONE),
        ..Default::default()
    };
    for loss in [0, 1, 2, 0] {
        enqueue(
            &mut pending,
            Incident::alert(&alert(&d, Some(-Decimal::from(loss))), &d),
            &limits,
        );
    }
    assert_eq!(pending.len(), 1);
    assert_eq!(pending["a.alert-1"].loss_usdt, Some(Decimal::ONE));
}

#[test]
fn failed_usdt_amount_is_not_a_loss_and_new_failed_attempt_has_its_own_key() {
    let mut d = deal();
    d.step = Step::Closed;
    d.result_usd = Some(Decimal::ONE);
    let at = d.at.clone();
    let amount = Decimal::from(219);
    let why = "повторы исчерпаны";
    let marker = crate::core::usdt_return_state::not_returned_text(&d.id, amount, &at, why);
    crate::core::usdt_return_parts::put(
        &mut d,
        UsdtReturn::NotReturned {
            at: at.clone(),
            amount,
            result_at_failure: None,
            why: why.into(),
        },
    );
    d.timeline.push((d.started_at, marker.clone()));
    let a = Incident::usdt(&d, &at, why, amount);
    d.note("Изменилась другая часть");
    assert_eq!(a.key, Incident::usdt(&d, &at, why, amount).key);
    assert_eq!(a.loss_usdt, Some(-Decimal::ONE));
    // Комиссия другой части изменила итог на −3 после появления проблемы первой части.
    let other = VenueId("gate".into());
    let to = d.direction.buy_on.clone();
    crate::core::usdt_return_parts::put(
        &mut d,
        UsdtReturn::Done {
            amount: Decimal::from(10),
            credited: Decimal::from(6),
            from: other,
            to,
            sent: None,
            fee_usd: None,
        },
    );
    crate::core::deal_return_cost::apply(&mut d);
    assert_eq!(d.result_usd, Some(Decimal::from(-3)));
    let mut stored = serde_json::to_value(&d).unwrap();
    let reopened: Deal = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(
        Incident::usdt(&reopened, &at, why, amount).loss_usdt,
        Some(-Decimal::ONE)
    );
    stored["usdt_return"]
        .as_object_mut()
        .unwrap()
        .remove("result_at_failure");
    let legacy: Deal = serde_json::from_value(stored).unwrap();
    assert_eq!(Incident::usdt(&legacy, &at, why, amount).loss_usdt, None);
    assert!(
        !Thresholds {
            usd: Some(Decimal::ONE),
            ..Default::default()
        }
        .qualifies(&a)
    );
    d.timeline
        .push((d.started_at + chrono::Duration::seconds(1), marker));
    assert_ne!(a.key, Incident::usdt(&d, &at, why, amount).key);
}

#[test]
fn halted_requires_unresolved_state_and_only_priced_pnl_can_raise_a_loss_alarm() {
    let mut d = deal();
    d.step = Step::Halted;
    assert!(!needs_review(&d));
    d.qty = Decimal::from(100);
    assert!(needs_review(&d));
    d.provisional_usd = Some(-d.invested);
    // Прежняя остановка без снимка и новая с неизвестным исходным PnL остаются без оценки.
    assert_eq!(Incident::halted(&d).loss_usdt, None);
    d.duty_halted_pnl_usdt = Some(Decimal::ONE);
    let limits = Thresholds {
        usd: Some(Decimal::from(2)),
        ..Default::default()
    };
    d.note(format!(
        "{}остановка",
        crate::core::deal_book::end_prefix(Step::Halted)
    ));
    let initial = Incident::halted(&d);
    assert!(!limits.qualifies(&initial));
    // Реконсиляция поменяла учётную оценку, но тот же эпизод был прибыльным в момент остановки.
    d.provisional_usd = Some(Decimal::from(-5));
    d.note("пересчёт количества при сверке");
    let saved: Deal = serde_json::from_value(serde_json::to_value(&d).unwrap()).unwrap();
    let recovered = Incident::halted(&saved);
    assert_eq!(recovered.key, initial.key);
    assert_eq!(recovered.loss_usdt, Some(-Decimal::ONE));
    assert!(!limits.qualifies(&recovered));
    for step in [Step::Closed, Step::TakenOver, Step::Buying] {
        d.step = step;
        assert!(!needs_review(&d));
    }
}

#[tokio::test]
async fn uncertain_delivery_and_per_chat_receipts_survive_reopen() {
    let path =
        std::env::temp_dir().join(format!("arb-duty-{}", crate::core::ids::uuid_v4().unwrap()));
    std::fs::create_dir_all(&path).unwrap();
    let file = path.join("test.sqlite");
    let d = deal();
    let i = Incident::alert(&alert(&d, Some(-Decimal::ONE)), &d);
    let key = i.key.clone();
    let mut entries = BTreeMap::from([(key.clone(), i)]);
    let db = crate::storage::db::Db::open(&file).unwrap();
    store::put(&db, &entries).await.unwrap();
    entries.get_mut(&key).unwrap().loss_usdt = Some(Decimal::from(7));
    store::put(&db, &entries).await.unwrap();
    delivery::confirm(&db, &key, "-2").await.unwrap();
    drop(db);
    let db = crate::storage::db::Db::open(&file).unwrap();
    assert_eq!(
        store::load(&db).await.unwrap()[&key].loss_usdt,
        Some(Decimal::ONE)
    );
    let recipients = delivery::recipients(&db, &key).await.unwrap();
    assert!(recipients.contains("-2"));
    assert!(!recipients.contains("-3"));
    store::remove(&db, vec![key]).await.unwrap();
    assert!(store::load(&db).await.unwrap().is_empty());
    drop(db);
    let _ = std::fs::remove_dir_all(path);
}
