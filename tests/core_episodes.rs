//! Файл: tests/core_episodes.rs
//! Что делает: эпизоды арбитражной ситуации (решение 74): (1) логика episode.rs по последовательности вердиктов —
//! открытие по первому прошедшему, пропуск чаще episode.min_interval_ms, запись пересчёта, закрытие с причиной по
//! отказу, повторное открытие — новый эпизод; отказ без открытого эпизода ничего не делает; (2) на настоящем ядре и
//! SQLite: сухой прошедший кандидат (записанные CAKE — size_common/calc.rs) через очередь кандидатов открывает эпизод,
//! карточка строится отдельной задачей, файл zstd получает заголовок, запись и закрытие, сводка в таблице episodes — с
//! причиной закрытия и наибольшей прибылью; не заданы episode.* — эпизоды не ведутся; (3) направления нет в справочнике
//! — уборка закрывает эпизод «направление больше не отслеживается».
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{episode, episode_run, card}, arb::storage::{episode_files, episodes_db}, tests/size_common.

mod size_common;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::episode::{EpisodeInfo, Episodes, Step};
use arb::core::episode_run::Accepted;
use arb::core::{candidates, episode_run};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::{episode_files, episodes_db};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use size_common::{Rec, d};
use tokio::sync::broadcast;

#[test]
fn verdict_sequence_opens_and_closes() {
    let dir = size_common::calc::direction();
    let mut e = Episodes::default();
    let t0 = Utc::now();
    let ms = |n| t0 + Duration::milliseconds(n);
    assert!(e.close(&dir, "отказ", t0).is_none(), "закрывать нечего");
    let Step::Opened(first) = e.accept(&dir, "e1".into(), true, d("1"), Some(d("3")), t0, 500)
    else {
        panic!("первый прошедший открывает")
    };
    assert!(first.dry && first.records == 1);
    assert_eq!(
        e.accept(&dir, "x".into(), true, d("5"), None, ms(100), 500),
        Step::Skip
    );
    assert!(!e.due(&dir, ms(100), 500) && e.due(&dir, ms(500), 500));
    let Step::Record(r) = e.accept(&dir, "x".into(), true, d("2"), Some(d("4")), ms(600), 500)
    else {
        panic!("пересчёт после интервала — запись")
    };
    assert_eq!((r.id.as_str(), r.records, r.max_profit), ("e1", 2, d("2")));
    assert_eq!(r.max_profit_unbounded, Some(d("4")));
    let closed = e.close(&dir, "прибыль ниже порога", ms(900)).unwrap();
    assert_eq!(closed.close_reason.as_deref(), Some("прибыль ниже порога"));
    assert_eq!(closed.ended_at, Some(ms(900)));
    assert!(!e.is_open(&dir));
    let Step::Opened(again) = e.accept(&dir, "e2".into(), false, d("1"), None, ms(1000), 500)
    else {
        panic!("после закрытия — новый эпизод")
    };
    assert_eq!(again.id, "e2");
}

/// Ядро на базе во временной папке.
async fn start(name: &str) -> (Core, Store, Db, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("arb-episode-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db: db.clone(),
        changes,
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    (core, settings, db, d)
}

/// Задаёт настройки эпизодов.
async fn episode_settings(s: &Store) {
    for (n, v) in [
        ("episode.book_levels", 5),
        ("episode.min_interval_ms", 0),
        ("episode.max_mb", 10),
        ("episode.keep_days", 7),
        ("episode.zstd_level", 3),
        ("episode.cleanup_s", 3600),
        ("dry.budget_usdt", 1000),
    ] {
        s.set(n, Scope::Global, Some(json!(v)), "тест")
            .await
            .unwrap();
    }
    s.set("dry.enabled", Scope::Global, Some(json!(true)), "тест")
        .await
        .unwrap();
}

/// Ждёт (не дольше 5 с), пока в базе у эпизода появится конец, и читает его файл до строки закрытия.
async fn closed_episode(db: &Db, dir: &std::path::Path) -> (EpisodeInfo, Vec<String>) {
    for _ in 0..50 {
        let rows = episodes_db::list(db, 10).await.unwrap();
        if let Some(info) = rows
            .iter()
            .map(|r| serde_json::from_str::<EpisodeInfo>(r).unwrap())
            .find(|e| e.ended_at.is_some())
        {
            let path = dir.join("episodes").join(format!("{}.jsonl.zst", info.id));
            if let Ok((l, _)) = episode_files::read(&path, 100)
                && l.last().is_some_and(|x| x.contains("\"close\""))
            {
                return (info, l);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("эпизод не закрылся в базе и файле за 5 с");
}

/// Путь очереди кандидатов (сухой режим): открытие, запись с карточкой (отдельная задача), закрытие отказом — файл
/// zstd и сводка в SQLite пишет задача эпизодов, вызывающие её не ждут.
#[tokio::test]
async fn episode_file_and_summary() {
    let (core, s, db, dir) = start("file").await;
    let rec = Rec::load("episode-file").await;
    let calc = size_common::calc::calc(&rec, d("10"));
    let d = calc.direction.clone();
    // Не заданы episode.* — эпизоды не ведутся.
    assert!(matches!(
        episode_run::on_accept(&core, &calc, true),
        Accepted::Off
    ));
    episode_settings(&s).await;
    let mode = arb::core::dry::mode(&s);
    candidates::accepted(&core, &d, calc.clone(), &mode).await;
    assert!(episode_run::is_open(&core, &d));
    // Карточка строится отдельной задачей — даём ей отдать запись, затем закрываем.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    episode_run::failed(&core, &d, "прибыль ниже порога");
    assert!(!episode_run::is_open(&core, &d));
    let (info, lines) = closed_episode(&db, &dir).await;
    assert_eq!(info.close_reason.as_deref(), Some("прибыль ниже порога"));
    assert!(info.dry);
    assert_eq!(info.max_profit, calc.profit_usd);
    let kinds: Vec<String> = lines
        .iter()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(kinds, ["open", "calc", "close"]);
    let rec_line: Value = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!(rec_line["card"]["ticker"], "CAKE");
    assert_eq!(rec_line["card"]["dry"], true);
}

/// Направления нет в справочнике (монета ушла) — эпизод закрывается уборкой с причиной, отметка сухого сигнала чистится.
#[tokio::test]
async fn sweep_closes_untracked_direction() {
    let (core, s, db, dir) = start("sweep").await;
    episode_settings(&s).await;
    let rec = Rec::load("episode-sweep").await;
    let calc = size_common::calc::calc(&rec, d("10"));
    let d = calc.direction.clone();
    assert!(matches!(
        episode_run::on_accept(&core, &calc, true),
        Accepted::Ep { first: true, .. }
    ));
    // Справочник ядра пуст (площадок нет) — направления CAKE в нём нет.
    episode_run::sweep(&core);
    assert!(!episode_run::is_open(&core, &d));
    let (info, _) = closed_episode(&db, &dir).await;
    assert_eq!(
        info.close_reason.as_deref(),
        Some("направление больше не отслеживается")
    );
}
