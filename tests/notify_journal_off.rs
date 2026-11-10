//! Файл: tests/notify_journal_off.rs
//! Что делает: решение 251, дополнение (Тигран 10.11) — выключенное событие уведомлений (notify.event.enabled = нет)
//! пишется в журнал одной записью (вид — код события, текст без разметки и пометка «в Telegram не отправлено»), если
//! у места события своей записи нет (notify/journal_off.rs OWN_RECORD); со своей записью — второй нет; не задано —
//! как раньше, без записи. Без обращения к Telegram (чат не задан), на настоящей SQLite во временной папке (моков нет).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::notify, arb::settings, arb::storage, arb::secrets.

use std::path::PathBuf;
use std::time::Duration;

use arb::notify::catalog;
use arb::notify::dispatch::Notifier;
use arb::notify::journal_off::{MARK, OWN_RECORD, wants_record};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::{Entry, Journal, Level};
use serde_json::json;
use tokio::sync::broadcast;

/// Отправитель уведомлений на базе во временной папке (Telegram не настроен).
async fn notifier(name: &str) -> Notifier {
    let d: PathBuf = std::env::temp_dir().join(format!("arb-joff-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Notifier {
        http: reqwest::Client::new(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("s.enc")),
        journal,
    }
}

/// Включает или выключает событие (область «событие»).
async fn enable(n: &Notifier, code: &str, on: bool) {
    n.settings
        .set(
            "notify.event.enabled",
            Scope::Event(code.into()),
            Some(json!(on)),
            "тест",
        )
        .await
        .unwrap();
}

/// Записи журнала с видом kind.
async fn of_kind(n: &Notifier, kind: &str) -> Vec<Entry> {
    let all = n.journal.list(1000, None).await.unwrap();
    all.into_iter().filter(|e| e.kind == kind).collect()
}

#[test]
fn own_record_table_is_from_catalog() {
    for c in OWN_RECORD {
        assert!(catalog::find(c).is_some(), "{c}: нет в каталоге");
    }
    let mut v: Vec<&str> = OWN_RECORD.to_vec();
    v.sort();
    v.dedup();
    assert_eq!(v.len(), OWN_RECORD.len(), "повтор в OWN_RECORD");
    // Таблица Тиграна 10.11: у этих своей записи нет — они должны писаться.
    for c in [
        "episode.opened",
        "episode.closed",
        "deal.buy_filled",
        "deal.sell_filled",
        "exec.verified",
        "bundle.assembled",
        "bundle.simulated",
        "bundle.sim_reverted",
        "bundle.sent",
        "bundle.included",
        "bundle.not_included",
        "dex.rebuilt_after_approve",
        "dex.swap_waits_inflow",
    ] {
        assert!(wants_record(c), "{c}");
    }
    assert!(!wants_record("deal.candidate"));
}

#[tokio::test]
async fn disabled_event_goes_to_journal_once() {
    let n = notifier("off").await;
    enable(&n, "episode.opened", false).await;
    let text = "Эпизод открыт: CAKE bingx → bsc_dex; прибыль 1.2345 USDT; id 42";
    n.send("episode.opened", Some("bingx"), text).await;
    let got = of_kind(&n, "episode.opened").await;
    assert_eq!(got.len(), 1, "одна запись");
    let e = &got[0];
    assert_eq!(e.venue.as_deref(), Some("bingx"));
    assert_eq!(e.level, Level::Info);
    let def = catalog::find("episode.opened").unwrap();
    let plain = arb::notify::dispatch::body("episode.opened", def, text);
    assert_eq!(e.message, format!("{plain}\n({MARK})"));
    assert!(!e.message.contains('<'), "без разметки: {}", e.message);
    let data = e.data.as_ref().unwrap();
    assert_eq!(data["telegram"], json!(false));
    assert_eq!(data["text"], json!(text));
    // В Telegram не пытались: записи notify («не отправлено: …») нет.
    assert!(of_kind(&n, "notify").await.is_empty());
}

#[tokio::test]
async fn own_record_and_unset_make_no_entry() {
    let n = notifier("own").await;
    // deal.candidate: место события само пишет core.candidate — второй записи нет.
    enable(&n, "deal.candidate", false).await;
    n.send("deal.candidate", Some("bingx"), "CAKE сигнал").await;
    assert!(of_kind(&n, "deal.candidate").await.is_empty());
    // Не задано (не «нет») — как раньше: ни записи, ни отправки.
    n.send("deal.buy_filled", Some("bingx"), "Куплено").await;
    assert!(of_kind(&n, "deal.buy_filled").await.is_empty());
    assert!(of_kind(&n, "notify").await.is_empty());
    // Включено, Telegram не настроен — прежняя запись «не отправлено», без пометки выключенного.
    enable(&n, "deal.buy_filled", true).await;
    n.send("deal.buy_filled", Some("bingx"), "Куплено").await;
    assert!(of_kind(&n, "deal.buy_filled").await.is_empty());
    assert_eq!(of_kind(&n, "notify").await.len(), 1);
}

#[tokio::test]
async fn stop_level_and_glued_event() {
    let n = notifier("glue").await;
    // Склеиваемое событие связи (notify/glue.rs) — своей задачей; выключенное — одна запись, уровень ⚠️ → Warn.
    enable(&n, "venue.disconnected", false).await;
    n.send("venue.disconnected", Some("mexc"), "поток стаканов закрыт")
        .await;
    let mut got = vec![];
    for _ in 0..50 {
        got = of_kind(&n, "venue.disconnected").await;
        if !got.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].level, Level::Warn);
    assert!(got[0].message.ends_with(&format!("({MARK})")));
}
