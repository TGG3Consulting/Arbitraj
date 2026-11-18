//! Файл: tests/t198_common/mod.rs
//! Что делает: решение 413 (t198) — общие помощники тестов распределения и факта: числа, записанный журнал живого
//! случая WKC → MEXC 18.11 (tests/data/wkc_mexc_2026-11-18/journal.json), ждущая доли, входы распределения; ядро на
//! SQLite во временной папке (площадок в реестре нет), сделка WKC, ждущая доли; код файла без комментариев.
//! Кто вызывает: tests/core_credit_split_t198.rs, tests/core_credit_split2_t198.rs, tests/core_credit_fact_t198.rs,
//! tests/core_credit_review3_t198.rs.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::gateway, serde_json, rust_decimal.

#![allow(dead_code)]

use std::path::PathBuf;

use arb::core::Core;
use arb::core::credit_fact::CreditFact;
use arb::core::credit_split::{Input, Pending};
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Число из строки. Ошибок нет (тест падает).
pub fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Записанный журнал живого случая.
pub fn journal() -> Value {
    let p = format!(
        "{}/tests/data/wkc_mexc_2026-11-18/journal.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Число журнала по пути JSON Pointer.
pub fn j(v: &Value, ptr: &str) -> Decimal {
    d(v.pointer(ptr).and_then(Value::as_str).unwrap())
}

/// Ждущая доли (снимок без времени).
pub fn pend(id: &str, record: Decimal, outsider: Option<Decimal>) -> Pending {
    Pending {
        id: id.into(),
        record,
        outsider,
        seen: None,
    }
}

/// Входы без удержания и допуска.
pub fn input(fact: Decimal, confirmed: Decimal, pending: Vec<Pending>) -> Input {
    Input {
        fact,
        confirmed,
        pending,
        unclear: false,
        hold_pct: Decimal::ZERO,
        tol_pct: Decimal::ZERO,
    }
}

/// Папка теста (новая).
pub fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-t198-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро на базе во временной папке; mexc, bingx — биржи, bsc_dex — DEX.
pub async fn start(name: &str) -> Core {
    let d = dir(name);
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
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![
        (VenueId("bingx".into()), VenueKind::Cex),
        (VenueId("mexc".into()), VenueKind::Cex),
        (VenueId("bsc_dex".into()), VenueKind::Dex),
    ];
    core
}

/// Сделка WKC DEX → `to`, зачисленная на `to` записью `record`, ждёт доли (шаг «продажа»).
pub fn credited(id: &str, to: &str, record: Decimal) -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "WKC".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bsc_dex".into()),
        sell_on: VenueId(to.into()),
    };
    let mut x = Deal::new(id.into(), dir, json!({}), d("18.22"));
    x.step = Step::Selling;
    x.at = VenueId(to.into());
    x.qty = record;
    x.invested = d("262.99");
    x.credit = Some(Box::new(CreditFact {
        venue: VenueId(to.into()),
        record,
        outsider: None,
        outsider_at: None,
        share: None,
        basis: None,
        at: chrono::Utc::now(),
        reread: None,
    }));
    x
}

/// Код без строк-комментариев (`//`). Ошибок нет (тест падает).
pub fn code_of(f: &str) -> String {
    let p = format!("{}/{f}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(p)
        .unwrap()
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}
