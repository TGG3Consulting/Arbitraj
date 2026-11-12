//! Файл: tests/report_common/mod.rs
//! Что делает: решение 257 — общее для тестов отчёта «Итоги сделок»: настоящее ядро на SQLite во временной папке (без
//! площадок в реестре, как тесты учёта), сделки из записанных строк отчёта 05.11
//! (tests/data/report_2026-11-05/deals.csv: старт, длительность, монета, площадки, вложено, факт, план) — новые (для
//! закрытия ядром) и в старом формате учёта: закрытые до 86832aa (хронология «Closed: …» — живой текст сделки
//! 20261104030213216TWT, docs/notes/usdt-return-close-2026-11-04.md; конец = старт + записанная длительность, итог —
//! записанный факт, начислений по дням нет).
//! Кто вызывает: tests/report_results.rs, tests/report_snap.rs.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets. Моков нет.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

/// Живая строка закрытия до 86832aa (docs/notes/usdt-return-close-2026-11-04.md, сделка TWT 04.11 03:02:35).
pub const OLD_CLOSE: &str = "Closed: продано (перегон не выгоднее продажи здесь)";

pub fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Папка теста (новая).
pub fn dir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("arb-res-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Ядро на базе в папке; хранилище настроек.
pub async fn start(p: &Path) -> (Core, Store) {
    let db = Db::open(&p.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(p.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: p.to_path_buf(),
    })
    .await;
    (core, settings)
}

/// Записанная строка: старт, длительность с, монета, покупка, продажа, вложено, факт, план.
pub type Rec = (
    chrono::DateTime<Utc>,
    i64,
    String,
    String,
    String,
    Decimal,
    Decimal,
    Decimal,
);

/// Записанные строки.
pub fn rows() -> Vec<Rec> {
    let v = |c: char| match c {
        'B' => "bingx",
        'D' => "bsc_dex",
        'M' => "mexc",
        'K' => "kucoin",
        _ => "gate",
    };
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/report_2026-11-05/deals.csv"
    );
    std::fs::read_to_string(p)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            let c: Vec<char> = f[3].chars().collect();
            let (md, hm) = f[0].split_once(' ').unwrap();
            let t = chrono::DateTime::parse_from_rfc3339(&format!("2026-{md}T{hm}:00Z")).unwrap();
            let (b, s) = (v(c[0]).to_string(), v(c[1]).to_string());
            (
                t.with_timezone(&Utc),
                f[1].parse().unwrap(),
                f[2].to_string(),
                b,
                s,
                d(f[4]),
                d(f[5]),
                d(f[6]),
            )
        })
        .collect()
}

/// Сделка записанной строки (вложено, выручка = вложено + факт, затрат 0 — в записи их нет); id — префикс + номер.
pub fn deal(k: usize, prefix: &str) -> Deal {
    let (t, _, coin, b, s, inv, fact, plan) = rows().swap_remove(k);
    let dir = Direction {
        asset: Asset {
            ticker: coin.clone(),
            bsc_contract: None,
        },
        buy_on: VenueId(b),
        sell_on: VenueId(s),
    };
    let mut x = Deal::new(format!("{prefix}{k:03}{coin}"), dir, json!(null), plan);
    x.started_at = t;
    x.invested = inv;
    x.proceeds = inv + fact;
    x
}

/// Все записанные сделки новыми (для закрытия ядром). Ошибок нет.
pub fn recorded(prefix: &str) -> Vec<Deal> {
    (0..rows().len()).map(|k| deal(k, prefix)).collect()
}

/// Записанные сделки в формате учёта до 86832aa: закрыты, итог — факт, конец — «Closed: …» в старт + длительность.
pub fn old_format() -> Vec<Deal> {
    rows()
        .iter()
        .enumerate()
        .map(|(k, (_, dur, ..))| {
            let mut x = deal(k, "o");
            x.step = Step::Closed;
            x.result_usd = Some(x.proceeds - x.invested);
            x.timeline
                .push((x.started_at + Duration::seconds(*dur), OLD_CLOSE.into()));
            x.updated_at = x.started_at + Duration::seconds(*dur) + Duration::hours(1);
            x
        })
        .collect()
}

/// Остановленная при перезапуске в формате до 86832aa (текст ledger_load.rs cf5073e: «Сделка <id> (<монета>, шаг <Step>,
/// <N> монет на <площадка>) прервана перезапуском — проверьте вручную»), оценка — минус вложено.
pub fn old_restart(k: usize, dur: i64) -> Deal {
    let mut x = deal(k, "oh");
    x.step = Step::Halted;
    x.provisional_usd = Some(-x.invested);
    let text = format!(
        "Сделка {} ({}, шаг Selling, 1 монет на bingx) прервана перезапуском — проверьте вручную",
        x.id, x.direction.asset.ticker
    );
    x.timeline
        .push((x.started_at + Duration::seconds(dur), text));
    x
}

/// Вчера: оценка −0.26 по сделке «old1» (запись daily_result, как у бота до решения 257).
pub async fn yesterday(p: &std::path::Path) {
    let db = Db::open(&p.join("t.sqlite")).unwrap();
    let y = (Utc::now().date_naive() - chrono::Duration::days(1)).to_string();
    let body = json!({"final_usd": "-0.26", "provisional": {"old1": ["-0.26", y]}}).to_string();
    db.call(move |c| {
        c.execute(
            "INSERT INTO daily_result (day, body, updated_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![y, body, "t"],
        )
        .map(|_| ())
    })
    .await
    .unwrap();
}
