//! Файл: tests/bsc_dex_head_stall.rs
//! Что делает: решение 282 — уведомление «узел BSC: блок не растёт» (node/head_stall.rs). Номера и время блоков — из
//! живых ответов eth_getBlockByNumber 2026-10-25 (tests/data/receipts_2026-10-25/responses.json, как в
//! tests/bsc_dex_head_age.rs). Проверяется:
//! - переходы (decide): порог не задан — ничего; номер стоит не дольше порога — ничего; дольше — одно «стоит», повтор
//!   круга — без второго сообщения; номер вырос — одно «снова идут» с простоем; порог сняли — состояние сброшено;
//! - текст: последний блок, время блока (UTC, из записанного milliTimestamp), когда бот увидел блок, секунды простоя;
//! - Stall::step на настоящей SQLite: без bsc.node_stall_alert_s — ни события ядру, ни записи журнала (как раньше);
//!   с ним — ровно по одному событию dex.node_stalled и dex.node_resumed в канал ядра и записи venue.head_stall;
//! - задача t122: событие не ушло (канала ядру нет) — «сообщено» не ставится, журнал — по записи каждого вида на
//!   простой; после появления канала — одно событие.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::node::{heads, head_stall, head_stall_text}, arb::settings, arb::storage. Моков нет.

use chrono::{Duration, TimeZone, Utc};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

use arb::gateway::events::MarketEvent;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::node::head_stall::{
    CODE_RESUMED, CODE_STALLED, Change, SETTING, Stall, decide,
};
use arb::venues::bsc_dex::node::head_stall_text::{text, utc_ms};
use arb::venues::bsc_dex::node::heads::Head;

/// Записанные блоки: (номер, milliTimestamp узла) по порядку номеров.
fn blocks() -> Vec<(u64, u64)> {
    let raw = std::fs::read_to_string("tests/data/receipts_2026-10-25/responses.json").unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    let hex =
        |x: &Value| u64::from_str_radix(x.as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    let mut out: Vec<(u64, u64)> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["request"]["method"] == "eth_getBlockByNumber")
        .filter_map(|r| r["response"]["result"].as_object().cloned())
        .map(|b| (hex(&b["number"]), hex(&b["milliTimestamp"])))
        .collect();
    out.sort();
    out.dedup_by_key(|b| b.0);
    out
}

/// Голова: номер записанного блока, «бот увидел» — время этого блока. Ошибок нет.
fn head(b: (u64, u64)) -> Head {
    Head {
        number: b.0,
        received_at: Utc.timestamp_millis_opt(b.1 as i64).unwrap(),
    }
}

#[test]
fn transitions_once_per_stall() {
    let bs = blocks();
    assert!(bs.len() >= 2, "в записи есть блоки");
    let (a, b) = (head(bs[0]), head(bs[bs.len() - 1]));
    let t0 = a.received_at;
    // Порог не задан — функции нет, даже если номер стоит час.
    assert_eq!(
        decide(None, Some(&a), None, t0 + Duration::hours(1)),
        (None, None)
    );
    // Нет головы — ничего.
    assert_eq!(decide(None, None, Some(30), t0), (None, None));
    // Ровно порог — ещё не простой («дольше порога»).
    assert_eq!(
        decide(None, Some(&a), Some(30), t0 + Duration::seconds(30)),
        (None, None)
    );
    // Дольше порога — одно «стоит».
    let now = t0 + Duration::milliseconds(30_001);
    let (told, c) = decide(None, Some(&a), Some(30), now);
    assert_eq!(told.as_ref(), Some(&a));
    assert_eq!(
        c,
        Some(Change::Stalled {
            head: a.clone(),
            stood_ms: 30_001
        })
    );
    // Следующие круги — без повторов, состояние держится.
    let (told2, c2) = decide(
        told.as_ref(),
        Some(&a),
        Some(30),
        now + Duration::minutes(10),
    );
    assert_eq!((told2.as_ref(), c2), (Some(&a), None));
    // Номер вырос — одно «снова идут», простой — от получения прежней до получения новой.
    let (told3, c3) = decide(
        told2.as_ref(),
        Some(&b),
        Some(30),
        now + Duration::minutes(11),
    );
    assert_eq!(told3, None);
    let idle = (b.received_at - a.received_at).num_milliseconds();
    assert_eq!(
        c3,
        Some(Change::Resumed {
            was: a.clone(),
            now: b.clone(),
            idle_ms: idle
        })
    );
    // Порог сняли во время простоя — состояние сброшено без сообщений.
    assert_eq!(decide(Some(&a), Some(&a), None, now), (None, None));
}

#[test]
fn text_has_block_time_and_seconds() {
    let bs = blocks();
    let a = head(bs[0]);
    let c = Change::Stalled {
        head: a.clone(),
        stood_ms: 75_400,
    };
    let t = text(&c, 60, Some(bs[0]), None, Some("Последний блок получен"));
    assert!(t.contains(&format!("Последний блок: {}", a.number)), "{t}");
    let bt = utc_ms(bs[0].1 as i64);
    assert!(
        bt.starts_with("2026-10-25 ") && bt.ends_with(" UTC"),
        "{bt}"
    );
    assert!(t.contains(&format!("Время блока: {bt}")), "{t}");
    assert!(
        t.contains("Номер не меняется: 75 с (порог bsc.node_stall_alert_s = 60 с)"),
        "{t}"
    );
    assert!(t.contains("узел отвечает, номер прежний"), "{t}");
    // Время другого блока в кэше — время этого блока не выдумывается.
    let other = text(&c, 60, Some(bs[bs.len() - 1]), Some("Timeout"), None);
    assert!(other.contains("Время блока: не получено"), "{other}");
    assert!(other.contains("ошибка — Timeout"), "{other}");
    let b = head(bs[bs.len() - 1]);
    let idle = (b.received_at - a.received_at).num_milliseconds();
    let r = text(
        &Change::Resumed {
            was: a.clone(),
            now: b.clone(),
            idle_ms: idle,
        },
        60,
        None,
        None,
        None,
    );
    assert!(r.contains(&format!("Новый блок: {}", b.number)), "{r}");
    assert!(r.contains(&format!("Простой: {} с", idle / 1000)), "{r}");
}

/// Окружение на настоящей SQLite (без настроек). Принимает: метку каталога. Ошибок нет (тест падает).
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-head-stall-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

/// Записи журнала venue.head_stall. Ошибок нет.
async fn said(ctx: &Ctx) -> Vec<String> {
    let all = ctx.journal.list(200, None).await.unwrap();
    all.into_iter()
        .filter(|e| e.kind == "venue.head_stall")
        .map(|e| e.message)
        .collect()
}

#[tokio::test]
async fn step_sends_one_event_each_way() {
    let ctx = ctx("step").await;
    let bs = blocks();
    let (tx, mut rx) = mpsc::channel(8);
    let st = Stall::default();
    st.set_out(tx);
    // Номер стоит 2 минуты, но порога нет — ни события, ни записи (поведение как до решения 282).
    let a = Head {
        number: bs[0].0,
        received_at: Utc::now() - Duration::minutes(2),
    };
    st.step(&ctx, Some(a.clone()), Some(bs[0]), None, None)
        .await;
    assert!(rx.try_recv().is_err());
    assert!(said(&ctx).await.is_empty());
    ctx.settings
        .set(SETTING, Scope::Global, Some(json!(60)), "тест")
        .await
        .unwrap();
    for _ in 0..3 {
        st.step(&ctx, Some(a.clone()), Some(bs[0]), None, None)
            .await;
    }
    let MarketEvent::Notice { code, text, venue } = rx.try_recv().unwrap() else {
        panic!("не Notice")
    };
    assert_eq!((code.as_str(), venue.0.as_str()), (CODE_STALLED, "bsc_dex"));
    assert!(
        text.contains(&format!("Последний блок: {}", a.number)),
        "{text}"
    );
    assert!(rx.try_recv().is_err(), "одно сообщение на простой");
    let b = Head {
        number: bs[bs.len() - 1].0,
        received_at: Utc::now(),
    };
    st.step(&ctx, Some(b.clone()), None, None, None).await;
    st.step(&ctx, Some(b.clone()), None, None, None).await;
    let MarketEvent::Notice { code, text, .. } = rx.try_recv().unwrap() else {
        panic!("не Notice")
    };
    assert_eq!(code, CODE_RESUMED);
    assert!(text.contains("Простой: 120 с"), "{text}");
    assert!(rx.try_recv().is_err(), "одно сообщение на восстановление");
    let j = said(&ctx).await;
    assert_eq!(j.len(), 2, "{j:?}");
    assert!(
        j.iter().any(|m| m.starts_with("Узел BSC: блок не растёт")),
        "{j:?}"
    );
    assert!(
        j.iter()
            .any(|m| m.starts_with("Узел BSC: блоки снова идут")),
        "{j:?}"
    );
}

/// Задача t122 (п. 5): «сообщено» ставится только после ушедшего события. Канала ядру ещё нет — событие не ушло,
/// состояние не меняется; за несколько кругов в журнале ровно одна запись простоя и одна «не ушло» (ревью t122);
/// канал появился — следующий круг сообщает тот же простой ровно один раз, без новой записи журнала. Блок — из живой
/// записи, SQLite настоящая.
#[tokio::test]
async fn step_retells_after_lost_send() {
    let ctx = ctx("lost").await;
    let bs = blocks();
    ctx.settings
        .set(SETTING, Scope::Global, Some(json!(60)), "тест")
        .await
        .unwrap();
    let st = Stall::default();
    let a = Head {
        number: bs[0].0,
        received_at: Utc::now() - Duration::minutes(2),
    };
    // Ревью t122: N кругов без канала — по одной записи каждого вида (текст простоя и «не ушло»), попытки — молча.
    for _ in 0..4 {
        st.step(&ctx, Some(a.clone()), Some(bs[0]), None, None)
            .await;
    }
    let j = said(&ctx).await;
    let count = |p: &str| j.iter().filter(|m| m.starts_with(p)).count();
    assert_eq!(count("Узел BSC: блок не растёт"), 1, "{j:?}");
    assert_eq!(count("Уведомление dex.node_stalled не ушло"), 1, "{j:?}");
    assert_eq!(j.len(), 2, "{j:?}");
    let (tx, mut rx) = mpsc::channel(8);
    st.set_out(tx);
    for _ in 0..3 {
        st.step(&ctx, Some(a.clone()), Some(bs[0]), None, None)
            .await;
    }
    let MarketEvent::Notice { code, .. } = rx.try_recv().unwrap() else {
        panic!("не Notice")
    };
    assert_eq!(code, CODE_STALLED);
    assert!(
        rx.try_recv().is_err(),
        "одно сообщение после появления канала"
    );
    assert_eq!(
        said(&ctx).await.len(),
        2,
        "успешная отправка журнал не дублирует"
    );
}
