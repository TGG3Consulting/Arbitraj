//! Файл: tests/core_usdt_return_close.rs
//! Что делает: решение 138 на настоящей SQLite во временной папке (ядро без площадок в реестре, как остальные тесты
//! учёта): закрытие сделки — общий шаг возврата USDT на площадку покупки. Живой случай 04.11 (20261104030213216TWT,
//! bingx → bsc_dex, «Перегон» → «продано здесь», 24.69 USDT остались на кошельке DEX): закрытие тем же вызовом, что
//! делает продажа (ledger::finish с «продано (перегон не выгоднее продажи здесь)»), запускает возврат; площадки в
//! реестре нет — перегон не состоялся — страховка «USDT не вернулись» (журнал, состояние сделки). «Продано вручную»
//! (решение 100) не на площадке покупки — страховка; «отдельно» и продажа на площадке покупки — возврата нет;
//! возврат, прерванный перезапуском, — «не вернулись». Страж по исходникам — tests/core_close_guard.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets. Моков нет.

use std::path::{Path, PathBuf};
use std::time::Duration;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::types::{
    Asset, CommandSource, DexAccountId, Direction, ManualAction, ManualCommand, VenueId,
};
use arb::core::usdt_return_state::UsdtReturn;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::{Entry, Journal};
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-ret-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро на базе в папке; usdt_return.mode = mode. Возвращает ядро и журнал.
async fn start(d: &Path, mode: &str) -> (Core, Journal) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    settings
        .set("usdt_return.mode", Scope::Global, Some(json!(mode)), "тест")
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
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, journal)
}

/// Сделка как живая TWT: куплено на buy, монеты (и выручка) на at, кошелёк DEX w1, вложено 25, выручка 24.69.
fn deal(id: &str, buy: &str, at: &str) -> Deal {
    let d = Direction {
        asset: Asset {
            ticker: "TWT".into(),
            bsc_contract: None,
        },
        buy_on: VenueId(buy.into()),
        sell_on: VenueId(at.into()),
    };
    let mut x = Deal::new(id.into(), d, json!(null), Decimal::ONE);
    x.at = VenueId(at.into());
    x.dex_account = Some(DexAccountId("w1".into()));
    x.invested = Decimal::from(25);
    x.proceeds = Decimal::new(2469, 2);
    x
}

/// Записи журнала с этим видом и подстрокой. Ошибок нет.
async fn entries(j: &Journal, kind: &str, part: &str) -> Vec<Entry> {
    let all = j.list(500, None).await.unwrap();
    all.into_iter()
        .filter(|e| e.kind == kind && e.message.contains(part))
        .collect()
}

/// Ждёт, пока состояние возврата сделки станет «не вернулись» (не дольше 5 с). Возвращает состояние.
async fn wait_not_returned(core: &Core, id: &str) -> Option<UsdtReturn> {
    for _ in 0..500 {
        let s = core.deals().into_iter().find(|d| d.id == id)?.usdt_return;
        if matches!(s, Some(UsdtReturn::NotReturned { .. })) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    None
}

#[tokio::test]
async fn relay_sold_here_close_starts_return_and_failure_alarms() {
    let (core, j) = start(&dir("relay"), "per_trade").await;
    let mut x = deal("t1TWT", "bingx", "bsc_dex");
    x.note("Застрявшая монета: режим relay");
    arb::core::ledger::finish(
        &core,
        &mut x,
        Step::Closed,
        "продано (перегон не выгоднее продажи здесь)",
    )
    .await;
    // Возврат начат тем же закрытием: состояние «идёт» и запись журнала с ID сделки.
    assert_eq!(
        x.usdt_return,
        Some(UsdtReturn::Pending {
            amount: Decimal::new(2469, 2),
            from: VenueId("bsc_dex".into()),
            to: VenueId("bingx".into()),
        })
    );
    assert_eq!(
        entries(&j, "core.usdt_return", "t1TWT: возврат USDT")
            .await
            .len(),
        1
    );
    // Площадок в реестре нет — перегон не состоялся: «не вернулись», где лежат и сколько.
    match wait_not_returned(&core, "t1TWT").await {
        Some(UsdtReturn::NotReturned { amount, at, why }) => {
            assert_eq!(amount, Decimal::new(2469, 2));
            assert_eq!(at, VenueId("bsc_dex".into()));
            assert!(why.contains("перегон не состоялся"), "{why}");
        }
        other => panic!("ждали «не вернулись», а {other:?}"),
    }
    // Страховка пишется в журнал сразу после смены состояния (отдельной записью) — ждём её, а не читаем
    // в тот же миг: иначе тест изредка падал на гонке (05.11).
    let mut alarm = Vec::new();
    for _ in 0..100 {
        alarm = entries(&j, "core.usdt_not_returned", "сделка t1TWT").await;
        if !alarm.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(alarm.len(), 1);
    assert!(alarm[0].message.contains("24.69 USDT лежат на DEX"));
}

#[tokio::test]
async fn sold_manually_elsewhere_alarms_without_relay() {
    let (core, j) = start(&dir("manual"), "per_trade").await;
    let mut x = deal("t2WKC", "bingx", "mexc");
    arb::core::ledger::finish(&core, &mut x, Step::Halted, "тест").await;
    let cmd = ManualCommand {
        action: ManualAction::SoldManually {
            deal_id: "t2WKC".into(),
            usdt: "20.05".into(),
        },
        source: CommandSource::Ui,
        by: Some("тест".into()),
        at: chrono::Utc::now(),
    };
    core.command(cmd).await.unwrap();
    let d = core.deals().into_iter().find(|d| d.id == "t2WKC").unwrap();
    assert_eq!(d.step, Step::Closed);
    match d.usdt_return {
        Some(UsdtReturn::NotReturned { amount, at, why }) => {
            assert_eq!(amount, Decimal::new(4474, 2));
            assert_eq!(at, VenueId("mexc".into()));
            assert!(why.contains("решение 100"), "{why}");
        }
        other => panic!("ждали «не вернулись», а {other:?}"),
    }
    assert_eq!(
        entries(&j, "core.usdt_not_returned", "сделка t2WKC")
            .await
            .len(),
        1
    );
    // Перегона не было: записи «возврат начат» нет.
    assert!(entries(&j, "core.usdt_return", "t2WKC").await.is_empty());
}

#[tokio::test]
async fn no_return_when_separate_or_sold_where_bought() {
    let (core, j) = start(&dir("none"), "separate").await;
    let mut x = deal("t3TWT", "bingx", "bsc_dex");
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано (выгодно)").await;
    assert_eq!(x.usdt_return, None);
    let (core2, j2) = start(&dir("same"), "per_trade").await;
    let mut y = deal("t4TWT", "bingx", "bingx");
    arb::core::ledger::finish(
        &core2,
        &mut y,
        Step::Closed,
        "частичная покупка: продать на A",
    )
    .await;
    assert_eq!(y.usdt_return, None);
    for (jj, id) in [(&j, "t3TWT"), (&j2, "t4TWT")] {
        assert!(entries(jj, "core.usdt_not_returned", id).await.is_empty());
        assert!(entries(jj, "core.usdt_return", id).await.is_empty());
    }
    drop(core);
}

#[tokio::test]
async fn pending_return_after_restart_is_not_returned() {
    let d = dir("restart");
    {
        let (core, _) = start(&d, "per_trade").await;
        let mut x = deal("t5TWT", "bingx", "bsc_dex");
        x.step = Step::Closed;
        x.usdt_return = Some(UsdtReturn::Pending {
            amount: Decimal::new(2469, 2),
            from: VenueId("bsc_dex".into()),
            to: VenueId("bingx".into()),
        });
        arb::core::ledger::save(&core, &x).await;
    }
    let (core, j) = start(&d, "per_trade").await;
    let x = core.deals().into_iter().find(|d| d.id == "t5TWT").unwrap();
    assert!(matches!(
        x.usdt_return,
        Some(UsdtReturn::NotReturned { .. })
    ));
    assert_eq!(
        entries(&j, "core.usdt_not_returned", "сделка t5TWT")
            .await
            .len(),
        1
    );
}
