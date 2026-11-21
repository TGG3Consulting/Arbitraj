//! Файл: tests/core_low_usdt_nodata.rs
//! Что делает: решение 140 — ложный алерт «Мало USDT … свободно 0» сразу после разблокировки. Список токенов адаптера DEX
//! приходит с подпиской рынка, до неё балансы кошелька — только строка BNB (venues/bsc_dex/wallet/balances.rs: all по
//! tokens.all()), и сверка считала USDT нулём. Теперь у кошелька DEX без строки USDT — «нет данных» (не 0, алерта и
//! перегона «отдельно» нет); строка есть и 0 — 0; у биржи строки нет — 0 (так биржа отвечает на нулевой баланс). Текст
//! алерта кошелька — «Кошелёк DEX «имя»: …» (на настоящей SQLite, ядро без площадок).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (low_usdt), arb::settings, arb::storage, arb::notify, arb::secrets. Моков нет.

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::low_usdt::free_of;
use arb::core::types::{Balance, DexAccountId, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

/// Строка баланса. Ошибок нет.
fn row(v: &str, asset: &str, free: i64) -> Balance {
    Balance {
        venue: VenueId(v.into()),
        asset: asset.into(),
        free: Decimal::from(free),
        locked: Decimal::ZERO,
    }
}

#[test]
fn dex_without_usdt_row_is_no_data() {
    // Сразу после разблокировки: кошелёк ответил только BNB (токенов подписки ещё нет).
    let just_bnb = vec![row("bsc_dex", "BNB", 1)];
    assert_eq!(free_of(VenueKind::Dex, &just_bnb, "USDT"), None);
    // Токен известен, баланс прочитан и равен 0 — это 0.
    let zero = vec![row("bsc_dex", "BNB", 1), row("bsc_dex", "USDT", 0)];
    assert_eq!(free_of(VenueKind::Dex, &zero, "USDT"), Some(Decimal::ZERO));
    // Биржа без строки USDT — ноль.
    assert_eq!(free_of(VenueKind::Cex, &[], "USDT"), Some(Decimal::ZERO));
    assert_eq!(
        free_of(VenueKind::Cex, &[row("bingx", "USDT", 7)], "USDT"),
        Some(Decimal::from(7))
    );
}

#[tokio::test]
async fn dex_wallet_alert_says_wallet() {
    let d = std::env::temp_dir().join(format!("arb-lowusdt-w-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    settings
        .set(
            "alert.min_usdt",
            Scope::Venue("bsc_dex".into()),
            Some(json!("10")),
            "тест",
        )
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
    let v = VenueId("bsc_dex".into());
    let w = DexAccountId("w1".into());
    arb::core::low_usdt::check(&core, &v, Some(&w), "bsc_dex «BingX»", Decimal::ONE, "USDT").await;
    let e = journal.list(100, None).await.unwrap();
    let alert: Vec<_> = e.iter().filter(|x| x.kind == "core.low_usdt").collect();
    assert_eq!(alert.len(), 1);
    // Кошелька w1 в маппинге теста нет — в подписи его id; с именем — «Кошелёк DEX «BingX»».
    assert!(
        alert[0]
            .message
            .starts_with("Кошелёк DEX «w1»: свободно 1 USDT"),
        "{}",
        alert[0].message
    );
}
