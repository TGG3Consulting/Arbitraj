use super::*;
use crate::report::sig_parts::File;
use chrono::TimeZone;

fn at(day: u32, h: u32, m: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, day, h, m, 0).unwrap()
}

fn file(n: u8) -> File {
    File {
        name: format!("test_part{n}.xlsx"),
        caption: "проверка".into(),
        bytes: vec![n],
    }
}

#[tokio::test]
async fn utc_schedule_first_enable_edit_and_restart() {
    let path = std::env::temp_dir().join(format!("arb-daily-clock-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let s = Spool::open(path.clone()).await.unwrap();
    let time = NaiveTime::from_hms_opt(15, 30, 0).unwrap();
    assert_eq!(s.slot(at(20, 16, 0), time).await.unwrap(), None);
    assert_eq!(s.slot(at(21, 15, 29), time).await.unwrap(), None);
    let slot = at(21, 15, 30);
    assert_eq!(s.slot(slot, time).await.unwrap(), Some(slot));
    let w = reports::window(slot, at(21, 17, 0));
    assert!(w.has(at(20, 15, 30)) && !w.has(slot));
    assert_eq!(w.to.unwrap() - w.from.unwrap(), TimeDelta::hours(24));
    let later = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
    assert_eq!(s.slot(at(21, 19, 0), later).await.unwrap(), Some(slot));
    drop(s);
    let s = Spool::open(path.clone()).await.unwrap();
    assert_eq!(
        s.slot(at(23, 0, 1), later).await.unwrap(),
        Some(at(22, 18, 0))
    );
    let midnight = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
    assert_eq!(
        s.slot(at(23, 0, 1), midnight).await.unwrap(),
        Some(at(23, 0, 0))
    );
    drop(s);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn unsealed_files_recover_and_ack_is_per_part_and_chat() {
    let path = std::env::temp_dir().join(format!("arb-daily-send-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let s = Spool::open(path.clone()).await.unwrap();
    let slot = at(21, 0, 0);
    assert_eq!(s.slot(slot, slot.time()).await.unwrap(), Some(slot));
    let now = slot.timestamp();
    assert!(s.begin("sig", now).await.unwrap());
    s.append("sig", file(9)).await.unwrap();
    let a = "-1001".to_string();
    let b = "-1002".to_string();
    assert!(s.pending(a.clone(), now).await.unwrap().is_empty());
    assert!(!s.begin("sig", now + 1).await.unwrap());
    assert!(s.begin("sig", now + 60).await.unwrap());
    for n in 1..=2 {
        s.append("sig", file(n)).await.unwrap();
    }
    s.seal("sig").await.unwrap();
    assert!(!s.begin("sig", now + 300).await.unwrap());
    let key = || "sig".to_string();
    let first = s.claim(a.clone(), key(), 1, now).await.unwrap().unwrap();
    assert_eq!(first.bytes, vec![1]);
    s.finish(a.clone(), key(), 1, "sent", 0).await.unwrap();
    assert_eq!(s.pending(a.clone(), now).await.unwrap(), vec![(key(), 2)]);
    assert_eq!(s.pending(b.clone(), now).await.unwrap().len(), 2);
    s.claim(a.clone(), key(), 2, now).await.unwrap().unwrap();
    s.finish(a.clone(), key(), 2, "rate_limited", now + 180)
        .await
        .unwrap();
    assert!(s.chat_waits(a.clone(), now + 100).await.unwrap());
    assert!(!s.chat_waits(b.clone(), now + 100).await.unwrap());
    assert!(s.pending(a.clone(), now + 100).await.unwrap().is_empty());
    s.claim(b.clone(), key(), 1, now).await.unwrap().unwrap();
    // Процесс упал до записи подтверждения Telegram.
    drop(s);
    let s = Spool::open(path.clone()).await.unwrap();
    assert_eq!(
        s.pending(a.clone(), now + 181).await.unwrap(),
        vec![(key(), 2)]
    );
    assert_eq!(
        s.claim(b, key(), 1, now + 181)
            .await
            .unwrap()
            .unwrap()
            .bytes,
        vec![1]
    );
    assert!(s.claim(a, key(), 1, now + 181).await.unwrap().is_none());
    drop(s);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn slot_freezes_recipients_and_releases_acknowledged_book() {
    let path = std::env::temp_dir().join(format!("arb-daily-chats-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let s = Spool::open(path.clone()).await.unwrap();
    let slot = at(21, 12, 4);
    let a = "1".to_string();
    let b = "2".to_string();
    s.slot_for(slot, slot.time(), vec![a.clone(), b.clone()])
        .await
        .unwrap();
    s.slot_for(
        slot + TimeDelta::hours(1),
        slot.time(),
        vec!["3".to_owned()],
    )
    .await
    .unwrap();
    assert_eq!(s.audience().await.unwrap(), vec![a.clone(), b.clone()]);
    s.begin("cap", 0).await.unwrap();
    s.append("cap", file(1)).await.unwrap();
    s.seal("cap").await.unwrap();
    for chat in [a, b] {
        assert!(
            s.claim(chat.clone(), "cap".into(), 1, 0)
                .await
                .unwrap()
                .is_some()
        );
        s.finish(chat, "cap".into(), 1, "sent", 0).await.unwrap();
    }
    assert!(s.pending("3".into(), 1).await.unwrap().is_empty());
    drop(s);
    std::fs::remove_file(path).unwrap();
}
