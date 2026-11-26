//! Проверяем всю сборку и подготовку к раздаче на настоящей SQLite, без подключения бирж и Telegram.
use super::{Archive, Core, REPORTS, Spool, reports};
use crate::{
    core::{
        deal::{Deal, Step},
        deps::Deps,
        types::{Asset, Direction, VenueId},
    },
    gateway::registry::Registry,
    notify::dispatch::Notifier,
    secrets::Vault,
    settings::store::Store,
    storage::{db::Db, journal::Journal},
};
use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

#[tokio::test]
async fn all_four_menu_reports_reach_two_recipients_with_the_same_window_and_bytes() {
    let path = std::env::temp_dir().join(format!("arb-four-daily-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let db = Db::open(&path.join("ledger-source.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(32);
    let journal = Journal::new(db.clone(), broadcast::channel(32).0);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        vault: Vault::locked(path.join("secrets.enc")),
        settings: settings.clone(),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 32,
        data_dir: path.clone(),
    })
    .await;
    // Первая записанная строка отчёта 05.11: CAKE, BingX → BSC, вложение 24.70, итог −0.26.
    let mut deal = Deal::new(
        "daily-fixture-CAKE".into(),
        Direction {
            asset: Asset {
                chain: None,
                ticker: "CAKE".into(),
                contract: None,
            },
            buy_on: VenueId("bingx".into()),
            sell_on: VenueId("bsc_dex".into()),
        },
        serde_json::Value::Null,
        "0.0341".parse().unwrap(),
    );
    deal.started_at = "2026-11-01T18:21:00Z".parse().unwrap();
    deal.updated_at = deal.started_at + chrono::TimeDelta::seconds(4027);
    deal.step = Step::Closed;
    deal.invested = "24.70".parse().unwrap();
    deal.proceeds = "24.44".parse().unwrap();
    deal.result_usd = Some(deal.proceeds - deal.invested);
    deal.timeline
        .push((deal.updated_at, "Closed: продано".into()));
    crate::core::persist::save(
        &core.inner.deps.db,
        crate::core::ledger::KIND,
        &deal.id,
        &deal,
    )
    .await
    .unwrap();

    let slot: DateTime<Utc> = "2026-11-02T08:15:00Z".parse().unwrap();
    let w = reports::window(slot, slot);
    assert!(w.has(deal.updated_at));
    let archive = Archive::open(&core).await.unwrap();
    assert!(
        archive
            .deals
            .iter()
            .any(|d| d.id == deal.id && d.result_usd == deal.result_usd)
    );
    let spool = Spool::open(path.join("delivery.sqlite")).await.unwrap();
    let a = "-1001".to_owned();
    let b = "-1002".to_owned();
    assert_eq!(
        spool
            .slot_for(slot, slot.time(), vec![a.clone(), b.clone()])
            .await
            .unwrap(),
        Some(slot)
    );
    let mut snapshot = None;
    for (key, _, _) in REPORTS {
        assert!(spool.begin(key, slot.timestamp()).await.unwrap());
        reports::build(&core, &archive, &spool, key, w, &mut snapshot)
            .await
            .unwrap();
    }
    let pending = spool.pending(a.clone(), slot.timestamp()).await.unwrap();
    assert_eq!(pending.len(), REPORTS.len());
    assert_eq!(
        pending,
        spool.pending(b.clone(), slot.timestamp()).await.unwrap()
    );
    for ((key, part), (expected, _, prefix)) in pending.into_iter().zip(REPORTS) {
        assert_eq!(key, expected);
        let first = spool
            .claim(a.clone(), key.clone(), part, slot.timestamp())
            .await
            .unwrap()
            .unwrap();
        let second = spool
            .claim(b.clone(), key, part, slot.timestamp())
            .await
            .unwrap()
            .unwrap();
        assert!(first.name.starts_with(prefix) && first.name.ends_with(".xlsx"));
        assert!(
            first.caption.contains("2026-11-01 08:15:00")
                && first.caption.contains("2026-11-02 08:15:00"),
            "{}",
            first.caption
        );
        assert!(first.bytes.starts_with(b"PK"));
        assert!(
            first
                .bytes
                .windows(b"xl/workbook.xml".len())
                .any(|s| s == b"xl/workbook.xml")
        );
        assert_eq!(first.bytes, second.bytes);
    }
    drop(spool);
    drop(archive);
    drop(core);
    let _ = std::fs::remove_dir_all(path);
}
