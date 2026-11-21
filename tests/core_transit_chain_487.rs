//! Файл: tests/core_transit_chain_487.rs
//! Что делает: решение 487 (T487-03) — статистика времени в пути ПАРЫ площадок (решение 464, core/transit_source.rs)
//! ведётся в сети перевода (Asset.chain): переводы сети по умолчанию (chain = None — BSC) и другой сети (chain =
//! arbitrum) одной пары gate → bsc_dex не смешиваются. История пары — запись живого бота 20.11.2026
//! tests/data/num_pool_2026-11-20/transits_gate_bsc_dex.json (/api/transits, строки gate → bsc_dex).
//! Проверки на настоящем ядре и SQLite во временной папке (площадок в реестре нет, моков нет):
//! - история BSC (chain = None) даёт монете BSC без своей истории статистику пары, монете той же пары в сети arbitrum —
//!   нет (настройки 37 + 2 = 39 с);
//! - перевод монеты arbitrum считается только в сумму arbitrum;
//! - перезапуск на той же базе: записи учёта без поля chain (вид до решения 416 — Asset без сети не пишет chain)
//!   читаются как сеть по умолчанию — сумма пары BSC та же, arbitrum — своя.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (transit, transit_source), arb::settings, arb::storage, tests/data/num_pool_2026-11-20.

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::transit;
use arb::core::transit_source::Source;
use arb::core::types::{Asset, ChainId, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Ядро на базе в папке `d` (как есть — для перезапуска). Настройки времени в пути — живого бота 20.11 (запись
/// transit_settings: manual_auto, порог 5, 37 с отправки gate, 2 с зачисления bsc_dex). Возвращает ядро.
async fn start_in(d: &std::path::Path) -> Core {
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
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![
        (VenueId("gate".into()), VenueKind::Cex),
        (VenueId("bsc_dex".into()), VenueKind::Dex),
    ];
    for (name, venue, v) in [
        ("profit.transit_mode", "gate", json!("manual_auto")),
        ("profit.transit_auto_after", "gate", json!(5)),
        ("profit.transit_manual_s", "gate", json!(37)),
        ("profit.transit_credit_s", "bsc_dex", json!(2)),
    ] {
        let at = Scope::Venue(venue.into());
        settings.set(name, at, Some(v), "тест").await.unwrap();
    }
    core
}

/// Монета записи в сети `chain` (None — сеть по умолчанию). Ошибок нет.
fn coin(ticker: &str, chain: Option<&str>) -> Asset {
    Asset {
        ticker: ticker.into(),
        chain: chain.map(|c| ChainId(c.into())),
        contract: None,
    }
}

/// Направление gate → bsc_dex монеты. Ошибок нет.
fn dir(a: Asset) -> Direction {
    Direction {
        asset: a,
        buy_on: VenueId("gate".into()),
        sell_on: VenueId("bsc_dex".into()),
    }
}

/// Строки записи /api/transits: (тикер, число переводов, среднее). Ошибок нет (тест падает без записи).
fn rows() -> Vec<(String, usize, Decimal)> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/num_pool_2026-11-20/transits_gate_bsc_dex.json"
    );
    let t: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    t["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let n = r["count"].as_u64().unwrap() as usize;
            let avg = r["avg_s"].as_str().unwrap().parse().unwrap();
            (r["ticker"].as_str().unwrap().to_string(), n, avg)
        })
        .collect()
}

#[tokio::test]
async fn pair_statistics_split_by_chain() {
    let d = std::env::temp_dir().join(format!("arb-487-transit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let core = start_in(&d).await;
    let (gate, dex) = (VenueId("gate".into()), VenueId("bsc_dex".into()));
    // История BSC (chain = None), кроме NUM — у NUM своей истории нет.
    let (mut count, mut coins) = (0usize, 0usize);
    for (t, n, avg) in rows().into_iter().filter(|(t, _, _)| t != "NUM") {
        for _ in 0..n {
            transit::record(&core, &coin(&t, None), &gate, &dex, avg).await;
        }
        count += n;
        coins += 1;
    }
    let bsc = dir(coin("NUM", None));
    let arb = dir(coin("NUM", Some("arbitrum")));
    let (_, src) = transit::seconds_from(&core, &bsc).unwrap();
    assert_eq!(src, Source::Pair(count, coins));
    // Та же пара в сети arbitrum: переводов BSC в её сумме нет — из настроек 37 + 2.
    let (secs, src) = transit::seconds_from(&core, &arb).unwrap();
    assert_eq!((secs, src), (Decimal::from(39), Source::Manual));
    // Перевод монеты arbitrum (время перевода NUM записи, 58,052 с) — только в сумму arbitrum.
    let t = "58.052".parse::<Decimal>().unwrap();
    transit::record(&core, &coin("USDT", Some("arbitrum")), &gate, &dex, t).await;
    let (_, src) = transit::seconds_from(&core, &bsc).unwrap();
    assert_eq!(src, Source::Pair(count, coins));
    // Перезапуск на той же базе: записи без chain — сеть по умолчанию, сумма BSC прежняя; у arbitrum 1 < 5 — настройки.
    drop(core);
    let core = start_in(&d).await;
    let (_, src) = transit::seconds_from(&core, &bsc).unwrap();
    assert_eq!(src, Source::Pair(count, coins));
    let (secs, src) = transit::seconds_from(&core, &arb).unwrap();
    assert_eq!((secs, src), (Decimal::from(39), Source::Manual));
}
