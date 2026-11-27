//! Граничные значения общего окна, повторное открытие базы и порядок публикации паузы.
use super::*;

#[test]
fn status_and_submit_consume_the_same_sliding_minute_and_backoff() {
    let mut w = Window::default();
    assert!(w.apply(&Action::Request(2), 10, "Ryze Relayer").is_ok());
    assert!(w.apply(&Action::Request(2), 20, "Ryze Relayer").is_ok());
    assert!(w.apply(&Action::Request(2), 60009, "Ryze Relayer").is_err());
    assert!(w.apply(&Action::Request(2), 60010, "Ryze Relayer").is_ok());
    assert!(w.apply(&Action::Pause(Some(90), None), 60010, "Ryze Relayer").is_ok());
    assert!(w.apply(&Action::Request(2), 150009, "Ryze Relayer").is_err());
    assert!(w.apply(&Action::Request(2), 150010, "Ryze Relayer").is_ok());
}
#[test]
fn late_reservation_cannot_shorten_observed_router_pause() {
    assert_eq!(deadline(123, None), 60_123);
    assert_eq!(deadline(123, Some(120)), 120_123);
    let origin = "http://ryze-window-test.invalid";
    block_router(origin, None);
    remember(&Key::Router(origin.into()), Window::default());
    assert!(!router_ready(origin, 100));
    forget_router(origin);
}
#[tokio::test]
async fn concurrent_wallets_and_reopen_share_the_budget() {
    let dir =
        crate::venues::bsc_dex::hashflow::recovery_tests::directory("ryze-relayer-limit", 8453);
    let path = dir.join("state.sqlite");
    {
        let db = Db::open(&path).unwrap();
        let (a, b) = tokio::join!(
            update(&db, Key::Relayer, Action::Request(1)),
            update(&db, Key::Relayer, Action::Request(1))
        );
        assert_ne!(a.is_ok(), b.is_ok());
    }
    let db = Db::open(&path).unwrap();
    assert!(update(&db, Key::Relayer, Action::Request(1)).await.is_err());
}
