//! Файл: tests/core_windows.rs
//! Что делает: окна (решение 125): (1) логика window.rs — окно монеты открывается первым прошедшим направлением,
//! второе направление той же монеты входит в то же окно (площадки — объединение), окно не закрывается, пока проходит
//! хоть одно направление, раньше window.min_hold_ms и пока идёт сделка; прошло снова в пределах срока — то же окно;
//! (2) список окон в настоящей SQLite — фильтры монеты, «со сделкой / без», «плюсовая / минусовая», страницы;
//! (3) на настоящем ядре, настоящем адаптере BingX (без сети) и записанных кадрах стакана BingX (tests/data/bingx):
//! прошедший кандидат (записанные CAKE — size_common/calc.rs) открывает окно, запись на BingX включается, кадры стакана
//! из потока адаптера уходят в файл окна сырыми с временем получения в нс, направление перестало проходить — окно
//! закрывается, сводка в SQLite, страница окна разбирает кадры адаптером (уровни и время BingX).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{window, window_run, candidates}, arb::storage::windows_db, arb::venues::bingx,
//! tests/size_common.

mod size_common;

use arb::core::Core;
use arb::core::coin_key::CoinKey;
use arb::core::deps::Deps;
use arb::core::types::{Direction, VenueId};
use arb::core::window::{Pass, WindowInfo, Windows};
use arb::core::{candidates, episode_run};
use arb::gateway::registry::Registry;
use arb::gateway::tap::{TapData, TapStream, now_ns};
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::windows_db::{self, Filter, Row};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use size_common::{Rec, d};
use tokio::sync::broadcast;

/// Направление CAKE BingX → MEXC (вторая биржа той же монеты).
fn cex_cex() -> Direction {
    let mut x = size_common::calc::direction();
    x.sell_on = VenueId("mexc".into());
    x
}

#[test]
fn coin_window_joins_holds_and_closes() {
    let (a, b) = (size_common::calc::direction(), cex_cex());
    let key = CoinKey::of(&a.asset);
    let mut w = Windows::default();
    let t0 = Utc::now();
    let ms = |n| t0 + Duration::milliseconds(n);
    let bingx = VenueId("bingx".into());
    let mexc = VenueId("mexc".into());
    let Pass::Opened(i) = w.pass(
        &a,
        "w1".into(),
        true,
        d("1"),
        std::slice::from_ref(&bingx),
        t0,
    ) else {
        panic!("первое прошедшее направление открывает окно")
    };
    assert_eq!(i.venues.len(), 1);
    assert_eq!(
        w.pass(
            &a,
            "x".into(),
            true,
            d("3"),
            std::slice::from_ref(&bingx),
            ms(10)
        ),
        Pass::Same
    );
    let Pass::Joined(j) = w.pass(
        &b,
        "x".into(),
        true,
        d("2"),
        &[bingx.clone(), mexc.clone()],
        ms(20),
    ) else {
        panic!("второе направление той же монеты — в то же окно")
    };
    assert_eq!(j.id, "w1");
    assert_eq!(
        j.venues
            .iter()
            .map(|v| v.venue.0.as_str())
            .collect::<Vec<_>>(),
        ["bingx", "mexc"]
    );
    assert_eq!(j.max_profit, d("3"));
    // Одно направление перестало проходить — второе держит окно.
    assert!(w.fail(&a, ms(30)));
    assert!(w.closable(ms(10_000), 100, |_| false).is_empty());
    assert!(w.fail(&b, ms(40)));
    assert_eq!(w.deadline(100), Some(ms(140)));
    assert!(
        w.closable(ms(139), 100, |_| false).is_empty(),
        "раньше window.min_hold_ms"
    );
    // Прошло снова в пределах срока — то же окно, срок снят.
    assert_eq!(
        w.pass(&a, "x".into(), true, d("1"), &[bingx], ms(100)),
        Pass::Same
    );
    assert_eq!(w.deadline(100), None);
    assert!(w.fail(&a, ms(200)));
    assert!(
        w.closable(ms(400), 100, |_| true).is_empty(),
        "сделка идёт — окно ждёт её конца"
    );
    assert_eq!(w.closable(ms(400), 100, |_| false), vec![key.clone()]);
    let c = w
        .close(&key, "направление больше не проходит", ms(400))
        .unwrap();
    assert_eq!((c.id.as_str(), c.ended_at), ("w1", Some(ms(400))));
    assert!(!w.fail(&a, ms(500)), "окна нет — нечего");
}

/// База во временной папке.
fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("arb-window-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn list_filters_and_pages() {
    let db = Db::open(&tmp("db").join("t.sqlite")).unwrap();
    let rows = [
        ("1", "CAKE", true, Some("plus")),
        ("2", "CAKE", false, None),
        ("3", "ETH", true, Some("minus")),
    ];
    for (n, t, deal, o) in rows {
        let r = Row {
            id: format!("w{n}"),
            started_at: format!("2026-11-03T10:00:0{n}Z"),
            ticker: t.into(),
            deal,
            outcome: o.map(str::to_string),
            body: json!({ "id": format!("w{n}") }).to_string(),
        };
        windows_db::save(&db, r).await.unwrap();
    }
    let ids = |v: Vec<String>| -> Vec<String> {
        v.iter()
            .map(|b| {
                serde_json::from_str::<Value>(b).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };
    let (all, n) = windows_db::page(&db, Filter::default(), 10, 0)
        .await
        .unwrap();
    assert_eq!(
        (ids(all), n),
        (vec!["w3".into(), "w2".into(), "w1".into()], 3)
    );
    let cake = Filter {
        ticker: Some("CAKE".into()),
        ..Default::default()
    };
    let (p, n) = windows_db::page(&db, cake.clone(), 1, 1).await.unwrap();
    assert_eq!(
        (ids(p), n),
        (vec!["w1".into()], 2),
        "вторая страница по одной строке"
    );
    let with = Filter {
        deal: Some(true),
        ..Default::default()
    };
    assert_eq!(windows_db::page(&db, with, 10, 0).await.unwrap().1, 2);
    let minus = Filter {
        outcome: Some("minus".into()),
        ..Default::default()
    };
    assert_eq!(
        ids(windows_db::page(&db, minus, 10, 0).await.unwrap().0),
        ["w3"]
    );
    let none = Filter {
        deal: Some(false),
        ..cake
    };
    assert_eq!(
        ids(windows_db::page(&db, none, 10, 0).await.unwrap().0),
        ["w2"]
    );
}

/// Настоящее ядро с настоящим адаптером BingX (ключей и адресов нет — сети нет). Возвращает и окружение адаптера:
/// его приёмники окон общие (Arc) — тест кладёт в них записанные кадры так же, как ws_conn.rs.
async fn start(d: &std::path::Path) -> (Core, Store, Db, arb::venues::bingx::ctx::Ctx) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let ctx = arb::venues::bingx::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw: arb::storage::raw_log::RawLog::start(d.to_path_buf(), settings.clone()),
        taps: Default::default(),
    };
    let gw = Registry::new();
    gw.register(std::sync::Arc::new(arb::venues::bingx::Bingx::new(
        ctx.clone(),
    )))
    .await;
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault,
        journal: journal.clone(),
    };
    let deps = Deps {
        gw,
        settings: settings.clone(),
        journal,
        notifier,
        db: db.clone(),
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    };
    (Core::start(deps).await, settings, db, ctx)
}

/// Записанные кадры стакана BingX (tests/data/bingx/ws_spot_2026-10-23.jsonl) — только стаканы.
fn book_frames() -> Vec<String> {
    let text = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    text.lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["m"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .filter(|m| m.contains("@depth") && m.contains("\"data\""))
        .collect()
}

#[tokio::test]
async fn window_records_raw_frames_and_closes() {
    let dir = tmp("e2e");
    let (core, s, db, ctx) = start(&dir).await;
    for (n, v) in [
        ("window.min_hold_ms", 0),
        ("window.queue_max", 100),
        ("window.max_mb", 10),
        ("window.keep_days", 7),
        ("window.zstd_level", 3),
        ("window.cleanup_s", 3600),
        ("dry.budget_usdt", 1000),
    ] {
        s.set(n, Scope::Global, Some(json!(v)), "тест")
            .await
            .unwrap();
    }
    for (n, v) in [
        ("window.enabled", json!(true)),
        ("dry.enabled", json!(true)),
    ] {
        s.set(n, Scope::Global, Some(v), "тест").await.unwrap();
    }
    let bx = Scope::Venue("bingx".into());
    s.set("venue.quote_asset", bx.clone(), Some(json!("USDT")), "тест")
        .await
        .unwrap();
    s.set("venue.depth_levels", bx, Some(json!(20)), "тест")
        .await
        .unwrap();
    let rec = Rec::load("window-e2e").await;
    let calc = size_common::calc::calc(&rec, d("10"));
    let dir_ = calc.direction.clone();
    let mode = arb::core::dry::mode(&s);
    candidates::accepted(&core, &dir_, calc.clone(), &mode).await;
    // Задача окон включает запись на BingX — ждём приёмник монеты.
    for _ in 0..50 {
        if ctx.taps.has("CAKE") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(ctx.taps.has("CAKE"), "запись CAKE на BingX не включилась");
    let frames = book_frames();
    assert!(frames.len() >= 2, "в записи есть стаканы");
    for f in &frames {
        ctx.taps
            .push("bingx", "CAKE", TapStream::Book, "t-conn", now_ns(), || {
                TapData::Text(f.clone())
            });
    }
    // Чужая монета не пишется.
    ctx.taps
        .push("bingx", "ETH", TapStream::Book, "t-conn", now_ns(), || {
            panic!("не пишется — не собирается")
        });
    episode_run::failed(&core, &dir_, "прибыль ниже порога");
    let info = closed(&db).await;
    assert!(!ctx.taps.has("CAKE"), "запись выключена при закрытии");
    assert_eq!(info.frames, frames.len() as u64);
    assert_eq!(info.lost, 0);
    assert!(info.dry && info.deals.is_empty() && info.outcome.is_none());
    assert_eq!(
        info.venues[0]
            .tap
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .book_channel,
        "CAKE-USDT@depth20"
    );
    // Строка закрытия дописывается потоком записи — ждём её в файле.
    let mut page = Value::Null;
    for _ in 0..50 {
        page = core.window(&info.id, 0, 100, 10).await.unwrap().unwrap();
        if page["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["type"] == "close")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let got = page["frames"].as_array().unwrap();
    assert_eq!(got.len(), frames.len());
    let first = &got[0]["decoded"]["Ok"];
    assert!(
        first["view"]["Book"]["asks"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "{first}"
    );
    assert!(
        first["exchange_ms"].as_i64().is_some(),
        "время BingX — поле timestamp"
    );
    assert!(got[0]["recv_ns"].as_i64().unwrap() > 1_700_000_000_000_000_000);
    let kinds: Vec<&str> = page["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["open", "tap", "close"]);
}

/// Ждёт (не дольше 5 с) закрытую сводку окна в базе.
async fn closed(db: &Db) -> WindowInfo {
    for _ in 0..50 {
        let (rows, _) = windows_db::page(db, Filter::default(), 10, 0)
            .await
            .unwrap();
        if let Some(w) = rows
            .iter()
            .map(|r| serde_json::from_str::<WindowInfo>(r).unwrap())
            .find(|w| w.ended_at.is_some())
        {
            return w;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("окно не закрылось за 5 с");
}
