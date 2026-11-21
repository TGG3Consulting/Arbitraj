//! Файл: tests/core_chain_ids_487.rs
//! Что делает: решение 487 (сети в id) — монета в id записей учёта, интерфейса и области настроек «монета» несёт сеть:
//! у монеты другой сети (Asset.chain = Some) — «…@сеть», у сети по умолчанию (chain = None) — id прежний. Монета —
//! NUM записи живого бота 20.11.2026 (tests/data/num_pool_2026-11-20/deals.json: адрес и перевод Gate → DEX — отправлено
//! 53440, зачислено 53203,82); та же монета в сети arbitrum — тот же адрес с chain = arbitrum (CREATE2 — один адрес в
//! двух сетях). На настоящем ядре и SQLite во временной папке (моков нет):
//! - записи учёта (core_state): факты вывода (withdraw_seen.rs, вид withdraw_fact), замер удержания при зачислении
//!   (deposit_hold.rs), статистика газа (gas_stats.rs, вид gas) — две записи, а не одна (до 487 вторая затирала первую);
//!   id записи BSC — прежний (адрес без «@»);
//! - карантин: id для «Снять карантин» монеты arbitrum разбирается в её ключ (quarantine.rs key_of_id), BSC — в свой;
//! - область «монета»: coin_key::scope_id — «0x…» у BSC, «0x…@arbitrum» у arbitrum; отметка coin.blocked одной сети не
//!   блокирует другую (blacklist::manual);
//! - id окна и эпизода (window_run / episode_run new_id) одного тикера в одну мс в двух сетях разные, у BSC — прежние.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (withdraw_seen, deposit_hold, gas_stats, quarantine, coin_key, blacklist, window_run,
//! episode_run, persist), arb::settings, arb::storage, tests/data/num_pool_2026-11-20.

use arb::core::Core;
use arb::core::coin_key::{self, CoinKey};
use arb::core::deps::Deps;
use arb::core::types::{Asset, ChainId, CoinGasStats, Direction, VenueId};
use arb::core::{
    blacklist, deposit_hold, episode_run, gas_stats, persist, quarantine, window_run, withdraw_seen,
};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Ядро на базе во временной папке. Возвращает ядро.
async fn start(name: &str) -> Core {
    let d = std::env::temp_dir().join(format!("arb-487-ids-{name}-{}", std::process::id()));
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
    Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d,
    })
    .await
}

/// Запись сделок NUM. Ошибок нет (тест падает без записи).
fn rec() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/num_pool_2026-11-20/deals.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// NUM записи в сети `chain` (None — сеть по умолчанию). Ошибок нет.
fn num(chain: Option<&str>) -> Asset {
    let r = rec();
    Asset {
        ticker: r["asset"]["ticker"].as_str().unwrap().into(),
        chain: chain.map(|c| ChainId(c.into())),
        contract: Some(
            r["asset"]["bsc_contract"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
        ),
    }
}

/// Десятичное поле сделки записи. Ошибок нет.
fn dv(k: &str) -> Decimal {
    rec()["deals"][0][k].as_str().unwrap().parse().unwrap()
}

#[test]
fn ids_carry_chain_only_for_other_network() {
    let (bsc, arb) = (num(None), num(Some("arbitrum")));
    let addr = format!("{:#x}", bsc.contract.unwrap());
    assert_eq!(coin_key::scope_id(&bsc), addr);
    assert_eq!(coin_key::scope_id(&arb), format!("{addr}@arbitrum"));
    assert_eq!(coin_key::rid(&bsc), bsc.contract.unwrap().to_string());
    assert_eq!(
        coin_key::rid(&arb),
        format!("{}@arbitrum", bsc.contract.unwrap())
    );
    // Карантин: id интерфейса → ключ своей сети.
    let back = |a: &Asset| quarantine::key_of_id(&quarantine::id_of(a));
    assert!(back(&bsc) == CoinKey::of(&bsc), "карантин BSC");
    assert!(back(&arb) == CoinKey::of(&arb), "карантин arbitrum");
    // Окно и эпизод одного тикера в одну мс: BSC — прежний вид, arbitrum — с сетью.
    let now = chrono::Utc::now();
    let d = |a: Asset| Direction {
        asset: a,
        buy_on: VenueId("gate".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let wb = window_run::new_id(&d(bsc.clone()), now);
    let wa = window_run::new_id(&d(arb.clone()), now);
    assert_eq!(wb, format!("{}-NUM", now.format("%Y%m%d%H%M%S%3f")));
    assert_ne!(wb, wa);
    let eb = episode_run::new_id(&d(bsc), now);
    let ea = episode_run::new_id(&d(arb), now);
    assert_eq!(
        eb,
        format!("{}-NUM-gate-bscdex", now.format("%Y%m%d%H%M%S%3f"))
    );
    assert_eq!(ea, format!("{eb}-arbitrum"));
}

#[tokio::test]
async fn records_of_two_networks_do_not_overwrite() {
    let core = start("records").await;
    let db = &core.inner.deps.db;
    let (bsc, arb) = (num(None), num(Some("arbitrum")));
    let gate = VenueId("gate".into());
    // Факты вывода (решение 253): перевод NUM записи — в обеих сетях.
    for a in [&bsc, &arb] {
        let s = withdraw_seen::Seen {
            asset: a.clone(),
            from: gate.clone(),
            sent: dv("sent"),
            credited: dv("credited"),
            fee: None,
        };
        withdraw_seen::record(&core, s).await;
    }
    let seen = persist::load::<withdraw_seen::Seen>(db, "withdraw_fact")
        .await
        .unwrap();
    assert_eq!(seen.len(), 2, "факты вывода BSC и arbitrum — две записи");
    // Замер удержания при зачислении (решение 413): выборка — запись и зачисление перевода записи.
    let pct = (dv("sent") - dv("credited")) / dv("sent") * Decimal::from(100);
    for a in [&bsc, &arb] {
        let s = deposit_hold::Sample {
            records: dv("sent"),
            credited: dv("credited"),
            pct,
            at: chrono::Utc::now(),
            deals: vec![rec()["deals"][0]["id"].as_str().unwrap().into()],
        };
        deposit_hold::put(&core, &gate, a, s);
        deposit_hold::save(&core, &gate, a).await;
    }
    let holds = persist::load::<deposit_hold::Hold>(db, "deposit_hold")
        .await
        .unwrap();
    let same = holds.iter().filter(|h| h.asset.contract == bsc.contract);
    assert_eq!(
        same.count(),
        2,
        "замер удержания BSC и arbitrum — две записи"
    );
    // Статистика газа: итог сделки по монете в обеих сетях.
    gas_stats::record_result(&core, &bsc, Decimal::ONE).await;
    gas_stats::record_result(&core, &arb, Decimal::ONE).await;
    let gas = persist::load::<CoinGasStats>(db, "gas").await.unwrap();
    assert_eq!(gas.len(), 2, "статистика газа BSC и arbitrum — две записи");
}

#[tokio::test]
async fn blocked_in_one_network_only() {
    let core = start("blocked").await;
    let s = &core.inner.deps.settings;
    let (bsc, arb) = (num(None), num(Some("arbitrum")));
    let at = Scope::Coin(coin_key::scope_id(&arb));
    s.set("coin.blocked", at, Some(json!(true)), "тест")
        .await
        .unwrap();
    assert!(
        blacklist::manual(s, &arb, &[]).is_some(),
        "arbitrum — в списке"
    );
    assert!(
        blacklist::manual(s, &bsc, &[]).is_none(),
        "BSC тем же адресом — нет"
    );
}
