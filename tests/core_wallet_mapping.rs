//! Файл: tests/core_wallet_mapping.rs
//! Что делает: маппинг кошельков в ядре (решение 132) на настоящем ядре, настоящих адаптерах BingX, MEXC и DEX (без
//! сети: используются только справочник видов площадок, маппинг и подпись) и SQLite во временной папке:
//! - направление BingX ↔ DEX берёт кошелёк строки bingx, адрес депозита на DEX — адрес ключа ЭТОГО кошелька;
//! - MEXC без строки маппинга: у MEXC ↔ DEX причина «нет кошелька DEX в маппинге», MEXC ↔ BingX не затронуто;
//! - циклы DEX ↔ DEX без строки не исполняются;
//! - сделка запоминает кошелёк: смена маппинга не меняет кошелёк идущей сделки; сделка до решения 132 — кошелёк
//!   переноса; незавершённые сделки кошелька не дают его удалить;
//! - перенос читает старую настройку исполнителя (записанное значение решения 132) и ставит маппинг bingx;
//! - решение 238 (ревью п. 2, дополнение — Тигран 09.11): котировка продажи сделки без кошелька — код NotConfigured.
//!
//! Ключи — официальные тестовые векторы (EIP-155 «Example», Hardhat Network счёт #0 — tests/bsc_dex_accounts.rs).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (dex_account, ledger), arb::venues (BingX, MEXC, DEX), arb::settings::accounts.

use alloy_primitives::{Address, address};
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use zeroize::Zeroizing;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::dex_account::{self, NO_WALLET};
use arb::core::types::{Asset, DexAccountId, Direction, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::secrets::crypto::KdfParams;
use arb::settings::accounts::ROW_DEX_DEX;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;

const KDF: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};
const KEY_BINGX: &str = "0x4646464646464646464646464646464646464646464646464646464646464646";
const KEY_MEXC: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const ADDR_MEXC: Address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
/// Записанный адрес исполнителя из решения 132 (exec.executor_address на сервере 03.11.2026).
const EXECUTOR: &str = "0x9ef558a5d437718dde44e00990e0efb9976cf794";

/// Ядро с тремя площадками; старая настройка исполнителя записана в базу как было до решения 132 (её уже нет в
/// каталоге — записать через хранилище нельзя). Возвращает: ядро, хранилище, секреты.
async fn start(tag: &str) -> (Core, Store, arb::secrets::Vault) {
    let d = std::env::temp_dir().join(format!("arb-core-wallets-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    db.call(|c| {
        c.execute(
            "INSERT INTO settings (name, scope, value, updated_at)
             VALUES ('exec.executor_address', 'venue:bsc_dex', ?1, '2026-11-03T00:00:00Z')",
            rusqlite::params![format!("\"{EXECUTOR}\"")],
        )
    })
    .await
    .unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let raw = RawLog::start(d.join("raw"), settings.clone());
    let gw = Registry::new();
    let bx = arb::venues::bingx::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
    };
    let mx = arb::venues::mexc::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
    };
    let dx = arb::venues::bsc_dex::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw,
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
    };
    gw.register(std::sync::Arc::new(arb::venues::bingx::Bingx::new(bx)))
        .await;
    gw.register(std::sync::Arc::new(arb::venues::mexc::Mexc::new(mx)))
        .await;
    gw.register(std::sync::Arc::new(arb::venues::bsc_dex::BscDex::new(dx)))
        .await;
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: vault.clone(),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw,
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    // Секреты: прежний ключ (станет кошельком «BingX» переносом) и ключ будущего кошелька MEXC.
    let pw = || Zeroizing::new("pw".to_string());
    vault.create(pw(), KDF).await.unwrap();
    let k = |s: &str| Some(Zeroizing::new(s.to_string()));
    vault
        .change(pw(), "bsc_dex.wallet_key", k(KEY_BINGX), KDF)
        .await
        .unwrap();
    vault
        .change(pw(), "wallet.w2.key", k(KEY_MEXC), KDF)
        .await
        .unwrap();
    settings
        .accounts()
        .configure_legacy(arb::settings::accounts::migrate::LegacyCfg {
            name: "BingX".into(),
            secret: "bsc_dex.wallet_key".into(),
            queue_key: "bsc_dex".into(),
            executor_setting: "exec.executor_address".into(),
            executor_scope: arb::settings::scope::Scope::Venue("bsc_dex".into()),
            rows: vec!["bingx".into(), arb::settings::accounts::ROW_PROBE.into()],
        });
    (core, settings, vault)
}

/// Направление монеты CAKE (адрес — PancakeSwap Token в BSC).
fn dir(buy: &str, sell: &str) -> Direction {
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82")),
        },
        buy_on: VenueId(buy.into()),
        sell_on: VenueId(sell.into()),
    }
}

#[tokio::test]
async fn mapping_picks_wallet_and_unmapped_exchange_does_not_trade() {
    let (core, s, _vault) = start("pick").await;
    let deps = &core.inner.deps;
    // Перенос: кошелёк «BingX», исполнитель — записанное значение старой настройки, строка bingx.
    let w = s
        .migrate_wallets(true, "тест")
        .await
        .unwrap()
        .expect("перенос");
    assert_eq!(w.executor.as_deref(), Some(EXECUTOR));
    let bingx = DexAccountId(w.id.clone());
    // BingX ↔ DEX (обе стороны) — кошелёк строки bingx.
    for d in [dir("bingx", "bsc_dex"), dir("bsc_dex", "bingx")] {
        assert_eq!(
            dex_account::of_direction(deps, &d).await.unwrap(),
            Some(bingx.clone())
        );
    }
    // MEXC без строки: причина — нет кошелька в маппинге; MEXC ↔ BingX — без кошелька, не затронуто.
    let e = dex_account::of_direction(deps, &dir("mexc", "bsc_dex"))
        .await
        .unwrap_err();
    assert!(e.contains(NO_WALLET), "{e}");
    assert!(
        dex_account::venue_dir(deps, &dir("bsc_dex", "mexc"), &VenueId("bsc_dex".into()))
            .await
            .is_err()
    );
    assert_eq!(
        dex_account::of_direction(deps, &dir("mexc", "bingx"))
            .await
            .unwrap(),
        None
    );
    // Циклы DEX ↔ DEX без строки — не исполняются.
    let e = dex_account::of_direction(deps, &dir("bsc_dex", "bsc_dex"))
        .await
        .unwrap_err();
    assert!(e.contains(ROW_DEX_DEX) && e.contains(NO_WALLET), "{e}");
    // Кошелёк MEXC добавлен и привязан — адрес депозита на DEX для MEXC ↔ DEX — адрес ключа MEXC.
    let a = s.accounts();
    let id = a.reserve_id().await.unwrap();
    assert_eq!(id, "w2", "ключ теста лежит под wallet.w2.key");
    a.add(&id, "MEXC", "wallet.w2.key", "wallet:w2", "тест")
        .await
        .unwrap();
    a.map_set("mexc", Some(&id), "тест").await.unwrap();
    let d = dir("mexc", "bsc_dex");
    let v = dex_account::venue_dir(deps, &d, &d.sell_on).await.unwrap();
    let addr = v.deposit_address(&d.asset, "BEP20").await.unwrap();
    assert_eq!(addr.parse::<Address>().unwrap(), ADDR_MEXC);
    let d = dir("bingx", "bsc_dex");
    let v = dex_account::venue_dir(deps, &d, &d.sell_on).await.unwrap();
    let addr = v.deposit_address(&d.asset, "BEP20").await.unwrap();
    assert_ne!(
        addr.parse::<Address>().unwrap(),
        ADDR_MEXC,
        "BingX ↔ DEX — свой кошелёк"
    );
}

#[tokio::test]
async fn deal_keeps_its_wallet_and_blocks_removal() {
    let (core, s, _vault) = start("deal").await;
    let w = s
        .migrate_wallets(true, "тест")
        .await
        .unwrap()
        .expect("перенос");
    let a = s.accounts();
    let id = a.reserve_id().await.unwrap();
    a.add(&id, "MEXC", "wallet.w2.key", "wallet:w2", "тест")
        .await
        .unwrap();
    // Сделка BingX ↔ DEX открыта с кошельком «BingX».
    let d = dir("bingx", "bsc_dex");
    let acc = dex_account::of_direction(&core.inner.deps, &d)
        .await
        .unwrap();
    let deal = arb::core::ledger::open(&core, &d, serde_json::json!({}), Decimal::ONE, acc);
    // Маппинг поменяли во время сделки — сделка идёт прежним кошельком.
    a.map_set("bingx", Some(&id), "тест").await.unwrap();
    let got = dex_account::of_deal(&core, &deal.id).unwrap();
    assert_eq!(got, Some(DexAccountId(w.id.clone())));
    // Кошелёк сделки нельзя удалить, пока сделка не завершена.
    a.map_set("bingx", None, "тест").await.unwrap();
    a.map_set(arb::settings::accounts::ROW_PROBE, None, "тест")
        .await
        .unwrap();
    let busy = dex_account::open_deals_of(&core, &w.id);
    assert_eq!(busy, vec![deal.id.clone()]);
    let e = a
        .remove(&w.id, &busy, "тест")
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains(&deal.id), "{e}");
    // Сделка до решения 132 (без кошелька) — кошелёк переноса.
    let old = arb::core::ledger::open(&core, &d, serde_json::json!({}), Decimal::ONE, None);
    assert_eq!(
        dex_account::of_deal(&core, &old.id).unwrap(),
        Some(DexAccountId(w.id.clone()))
    );
    // Биржа ↔ биржа — без кошелька.
    let cex = arb::core::ledger::open(
        &core,
        &dir("bingx", "mexc"),
        serde_json::json!({}),
        Decimal::ONE,
        None,
    );
    assert_eq!(dex_account::of_deal(&core, &cex.id).unwrap(), None);
}

/// Решение 238 (ревью, п. 2; дополнение — Тигран 09.11: согласовано): котировка продажи сделки, монета которой на DEX, —
/// привязка кошелька не задана: (а) сделка без счёта и без кошелька переноса (перенос не сделан) — dex_account::of;
/// (б) счёт сделки — кошелёк, которого нет (шлюз get_as → for_account). Обе — код NotConfigured (застрявшая монета
/// сразу «Остановлена», stuck_quote.rs), текст — прежний.
#[tokio::test]
async fn deal_quote_without_wallet_is_not_configured() {
    use arb::gateway::errors::ErrorCode;
    let (core, _s, _vault) = start("nowallet").await;
    let d = dir("bingx", "bsc_dex");
    let mut deal = arb::core::deal::Deal::new(
        "20261109000000021CAKE".into(),
        d,
        serde_json::json!(null),
        Decimal::ZERO,
    );
    deal.at = VenueId("bsc_dex".into());
    deal.qty = Decimal::ONE;
    let code = |r: Result<_, arb::core::op_error::OpError>| match r {
        Err(e) => (e.code, e.message),
        Ok(_) => panic!("котировка без кошелька"),
    };
    let (c, m) = code(
        arb::core::trade_ops::deal_quote(&core, &deal)
            .await
            .map(|_| ()),
    );
    assert_eq!(c, Some(ErrorCode::NotConfigured), "{m}");
    assert!(m.contains(NO_WALLET), "{m}");
    deal.dex_account = Some(DexAccountId("w-none".into()));
    let (c, m) = code(
        arb::core::trade_ops::deal_quote(&core, &deal)
            .await
            .map(|_| ()),
    );
    assert_eq!(c, Some(ErrorCode::NotConfigured), "{m}");
    assert!(m.contains("w-none"), "{m}");
    // Текстовая обёртка (deal_value — прочие вызывающие) — тот же текст.
    let t = arb::core::trade_ops::deal_value(&core, &deal)
        .await
        .unwrap_err();
    assert_eq!(t, m);
}
