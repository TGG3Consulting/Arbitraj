//! Файл: tests/feed_watch_venues.rs
//! Что делает: решения 202/213 «для всех бирж» (Тигран 08.11) — стык сторожа пар у адаптеров MEXC и Bitget на настоящих
//! адаптерах (без сети), настоящем хранилище настроек (SQLite во временной папке) и живой логике ядра
//! (core/feed_watch_logic.rs decide):
//! - feed_health есть (сторож включён): кадры не по расписанию, переподписка одной пары есть, подтверждение по REST
//!   есть — по содержимому стакана (Тигран 08.11; book_print.rs адаптеров, core/feed_print.rs): book_version без
//!   venue.depth_levels / без пары в справочнике отвечает ошибкой с причиной, а не NotApplicable;
//! - до подписки соединений нет — команда сторожа (переподписать пару, переподключить соединение) отвечает
//!   BadResponse с причиной, а не молча;
//! - ядро с такими признаками молчащую пару вслепую не переподписывает: при заданных venue.quiet_confirm_* — спрашивает
//!   биржу (Check), без них — только журнал «тихие пары» (QuietNoConfirm) с причиной.
//!
//! Кадры стакана WebSocket MEXC (protobuf limit.depth) и Bitget (books15) в tests/data не записаны — проверка отметки
//! кадров в учёте свежести на живых кадрах ждёт записи (tests/README.md; tests/mexc_window_tap.rs,
//! tests/bitget_real_data.rs recorded_ws_frames). Моков нет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::{mexc, bitget}, arb::gateway (contract, feed_health, errors), arb::core::feed_watch_logic.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::broadcast;

use arb::core::feed_confirm::QuietCfg;
use arb::core::feed_watch_logic::{Action, Cfg, State, decide};
use arb::gateway::contract::Venue;
use arb::gateway::errors::ErrorCode;
use arb::gateway::feed_health::{ConnHealth, FeedHealth, PairHealth};
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;

/// Адаптеры MEXC и Bitget на хранилище во временной папке (без сети и секретов).
async fn venues(tag: &str) -> Vec<Arc<dyn Venue>> {
    let d = std::env::temp_dir().join(format!("arb-feed-watch-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let raw = RawLog::start(d.join("raw"), settings.clone());
    let mx = arb::venues::mexc::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
    };
    let bg = arb::venues::bitget::ctx::Ctx {
        settings,
        vault,
        http: reqwest::Client::new(),
        journal,
        raw,
    };
    vec![
        Arc::new(arb::venues::mexc::Mexc::new(mx)),
        Arc::new(arb::venues::bitget::Bitget::new(bg)),
    ]
}

#[tokio::test]
async fn mexc_bitget_feed_contract() {
    for v in venues("contract").await {
        let id = v.id().0;
        let h = v.feed_health().expect("сторож включён");
        assert!(!h.scheduled && h.per_pair && h.confirm, "{id}: {h:?}");
        assert!(h.conns.is_empty(), "{id}: до подписки соединений нет");
        let e = v.resubscribe_pair("market-1", "CAKE").await.unwrap_err();
        assert_eq!(e.code, ErrorCode::BadResponse, "{id}");
        assert!(e.message.contains("market-1"), "{id}: {}", e.message);
        let e = v.reconnect_conn("market-1").await.unwrap_err();
        assert_eq!(e.code, ErrorCode::BadResponse, "{id}");
        // Подтверждение по содержимому есть: без настройки/справочника — ошибка с причиной, без сети.
        let e = v.book_version("CAKE").await.unwrap_err();
        match id.as_str() {
            // venue.depth_levels (N уровней) не задана.
            "mexc" => assert_eq!(e.code, ErrorCode::NotConfigured, "{id}: {}", e.message),
            // Пары нет в справочнике (подписки не было) — символа нет.
            _ => assert_eq!(e.code, ErrorCode::BadResponse, "{id}: {}", e.message),
        }
    }
}

/// Числа — как в core/feed_watch_tests.rs (советы docs/FIRST_RUN.md); подтверждение по REST задано.
fn cfg() -> Cfg {
    Cfg {
        book_stale_ms: 5_000,
        dead_mult: 3,
        live_ms: 40_000,
        warmup_ms: 30_000,
        attempts: 3,
        interval_ms: 3_000,
        pause_ms: 60_000,
        dead_pct: 30,
        reconnect_min_ms: 120_000,
        reconnect_max_per_hour: 3,
        quiet: QuietCfg::Confirm {
            min_ms: 60_000,
            max_per_s: 2,
        },
    }
}

#[tokio::test]
async fn mexc_bitget_quiet_pair_checked_or_journal() {
    for v in venues("quiet").await {
        let id = v.id().0;
        let flags = v.feed_health().expect("сторож включён");
        let now = 200_000;
        let pair = |t: &str, last: i64| PairHealth {
            ticker: t.into(),
            last_ms: Some(last),
            since_ms: 0,
            last_id: None,
            last_changed_ms: None,
        };
        let h = FeedHealth {
            conns: vec![ConnHealth {
                conn: "market-1".into(),
                alive_ms: Some(now),
                started_ms: Some(0),
                pairs: vec![pair("CAKE", now - 100_000), pair("BNB", now - 10)],
            }],
            ..flags
        };
        // venue.quiet_confirm_* заданы — спросить биржу (book_version), не переподписывать вслепую.
        let mut st = State::default();
        let acts = decide(&h, now, &cfg(), &HashMap::new(), &mut st);
        assert_eq!(
            acts,
            vec![Action::Check {
                conn: "market-1".into(),
                ticker: "CAKE".into(),
            }],
            "{id}"
        );
        // Не заданы — только журнал, раз на пару.
        let off = Cfg {
            quiet: QuietCfg::Off("не задана venue.quiet_confirm_min_ms".into()),
            ..cfg()
        };
        let mut st = State::default();
        let acts = decide(&h, now, &off, &HashMap::new(), &mut st);
        assert!(
            matches!(&acts[..], [Action::QuietNoConfirm { why, tickers, .. }]
                if why.contains("quiet_confirm_min_ms") && tickers == &vec!["CAKE".to_string()]),
            "{id}: {acts:?}"
        );
        assert!(
            decide(&h, now + 5_000, &off, &HashMap::new(), &mut st).is_empty(),
            "{id}"
        );
    }
}
