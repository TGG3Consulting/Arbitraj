//! Файл: tests/notify_251_followup.rs
//! Что делает: решение 251, ответы Тиграна 10.11 (б) и (г), без обращения к Telegram, на настоящей SQLite во временной
//! папке (моков нет):
//! (б) сообщения «Ручного управления» переживают перезапуск: память → база (storage/notify_mirror.rs) → новая память;
//!     запись снова на экране с тем же текстом — ни нового сообщения, ни правки; сменился текст — правка; записи нет —
//!     «ушла»; строки другого чата — к удалению; запись ушла — строка удаляется;
//! (г) deal.closed / stuck.loss_alert не в «Общий», если задан чат «Сделки» / «Неудачные» (routes::not_general);
//!     склейка событий связи по площадке: первое — сразу, пришедшие во время отправки — следующей одной пачкой.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (типы, manual_rows), arb::notify, arb::settings, arb::storage.

use std::time::Instant;

use arb::core::manual_rows::alert_record;
use arb::core::types::{Asset, ManualRecord, StuckAlert, VenueId};
use arb::notify::glue::{self, Item};
use arb::notify::mirror_state::MirrorState;
use arb::notify::roles::Role;
use arb::notify::routes::not_general;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::notify_mirror;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

/// База во временной папке.
fn db(name: &str) -> (Db, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("arb-251f-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    (Db::open(&d.join("t.sqlite")).unwrap(), d)
}

/// Алерт сделки CAKE (как tests/notify_mirror.rs) с заданным id и причиной.
fn rec(id: &str, reason: &str) -> ManualRecord {
    let a = StuckAlert {
        id: id.into(),
        deal_id: "20261104120000123CAKE".into(),
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        qty: Decimal::new(1234, 2),
        loss_pct: Some(Decimal::new(-35, 1)),
        reason: Some(reason.into()),
        at: Utc::now(),
        error: None,
        attempts: 1,
    };
    alert_record(&a, Decimal::ZERO)
}

/// Память после первой отправки записей в чат chat.
fn sent_state(chat: &str, recs: &[ManualRecord]) -> MirrorState {
    let mut st = MirrorState::default();
    assert!(!st.switch_chat(chat));
    st.apply(recs);
    for (i, r) in recs.iter().enumerate() {
        st.mark_sent(&r.key, r.text.clone(), 100 + i as i64, Instant::now());
    }
    st
}

#[tokio::test]
async fn mirror_messages_survive_restart() {
    let (db, _d) = db("mirror");
    let (a, b, c) = (
        rec("a1", "убыток 3.5%"),
        rec("b2", "убыток 4%"),
        rec("c3", "убыток 5%"),
    );
    let st = sent_state("-100", &[a.clone(), b.clone(), c.clone()]);
    let want = st.rows();
    let (put, del) = st.store_plan(&want);
    assert_eq!((put.len(), del.len()), (3, 0));
    notify_mirror::apply(&db, put, del).await.unwrap();
    // Строка чужого чата (чат зеркала сменили, пока бот стоял) и нечитаемая — к удалению.
    let mut foreign = sent_state("-200", &[rec("z9", "x")]).rows();
    notify_mirror::apply(
        &db,
        foreign
            .drain()
            .chain([("bad".into(), "{".into())])
            .collect(),
        vec![],
    )
    .await
    .unwrap();

    // Перезапуск: новая память из базы.
    let rows = notify_mirror::load_all(&db).await.unwrap();
    let mut st2 = MirrorState::default();
    let mut del = st2.restore(rows, "-100", Instant::now());
    del.sort();
    let mut expect = vec![rec("z9", "x").key, "bad".to_string()];
    expect.sort();
    assert_eq!(del, expect);
    assert!(
        !st2.switch_chat("-100"),
        "чат тот же — память не сбрасывается"
    );
    assert_eq!(st2.sent[&a.key].msgs, vec![100]);
    // a — та же, b — новый текст, c — ушла с экрана.
    let b2 = rec("b2", "убыток 6%");
    let diff = st2.apply(&[a.clone(), b2.clone()]);
    assert!(
        st2.unsent(&[a.clone(), b2.clone()]).is_empty(),
        "новых сообщений нет"
    );
    assert_eq!(
        diff.changed,
        vec![b2.clone()],
        "правится только сменившаяся"
    );
    assert_eq!(diff.gone.len(), 1);
    assert_eq!(diff.gone[0].0.key, c.key);
    assert_eq!(diff.gone[0].1.msgs, vec![102]);
    // Напоминания продолжаются после перезапуска.
    assert!(st2.last_reminder.is_some());
    // Правка не прошла — новое сообщение: в базе новый id; ушедшая c — строка удаляется.
    st2.replace_msg(&b2.key, b2.text.clone(), 555);
    let want = st2.rows();
    let (put, del) = st2.store_plan(&want);
    assert_eq!(del, vec![c.key.clone()]);
    assert_eq!(put.len(), 1, "a не менялась — не переписывается");
    assert!(put[0].1.contains("555"));
}

#[tokio::test]
async fn closed_deal_and_loss_alert_leave_general_when_role_chat_set() {
    let (db, d) = db("routes");
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let s = Store::load(db, changes, journal).await.unwrap();
    let _ = d;
    // Чаты ролей не заданы — в «Общий», как раньше.
    assert_eq!(not_general(&s, "deal.closed"), None);
    assert_eq!(not_general(&s, "stuck.loss_alert"), None);
    let set = |k: &'static str, v: &'static str| {
        let s = s.clone();
        async move {
            s.set(k, Scope::Global, Some(json!(v)), "тест")
                .await
                .unwrap()
        }
    };
    set("notify.deals_chat", "-300").await;
    assert_eq!(not_general(&s, "deal.closed"), Some(Role::Deals));
    assert_eq!(not_general(&s, "stuck.loss_alert"), None);
    set("notify.failures_chat", "-400").await;
    assert_eq!(not_general(&s, "stuck.loss_alert"), Some(Role::Failures));
    // Остальные события — в «Общий».
    assert_eq!(not_general(&s, "risk.stop"), None);
}

#[test]
fn venue_link_events_glue_per_venue() {
    let it = |code: &str, text: &str| Item {
        code: code.into(),
        venue: Some("t251_bingx".into()),
        text: text.into(),
    };
    assert!(glue::glued("venue.rate_limited") && glue::glued("dex.receipts_stuck"));
    assert!(!glue::glued("risk.stop"));
    // Первое — сразу (вызывающий начинает отправку).
    assert!(glue::enter("t251_bingx", it("venue.rate_limited", "1")).is_some());
    // Пока идёт отправка — копятся; другая площадка — независимо.
    assert!(glue::enter("t251_bingx", it("venue.feed_conn_blocked", "2")).is_none());
    assert!(glue::enter("t251_bingx", it("venue.rate_limited", "3")).is_none());
    assert!(glue::enter("t251_mexc", it("venue.rate_limited", "x")).is_some());
    // Отправка кончилась — следующая пачка одним сообщением; потом пусто — площадка свободна.
    let batch = glue::next("t251_bingx");
    assert_eq!(batch.len(), 2);
    assert!(glue::next("t251_bingx").is_empty());
    assert!(glue::enter("t251_bingx", it("venue.rate_limited", "4")).is_some());
    let text = glue::batch_text("bingx", &batch);
    assert!(text.starts_with("⚠️ BingX: события связи — 2"), "{text}");
    assert!(text.contains('2') && text.contains('3'), "{text}");
}
