//! Файл: tests/core_wallet_pause.rs
//! Что проверяет: решение 230 (Тигран 08.11.2026) на живом ядре (настоящая SQLite во временной папке, площадок в
//! реестре нет — только справочник видов площадок и маппинг кошельков, моков нет):
//! 1) расхождение сверки кошелька DEX ставит паузу ТОЛЬКО этому кошельку (Risk.wallet_paused), площадка bsc_dex не
//!    на паузе; расхождение биржи — как раньше пауза площадки (решение 221);
//! 2) risk::can_open отказывает только направлениям этого кошелька (маппинг биржи → кошелёк, решение 132; DEX ↔ DEX —
//!    свой кошелёк), остальные кошельки торгуют;
//! 3) автоснятие — ledger.reconcile_confirm_rounds чистых кругов подряд ЭТОГО кошелька: проблема другого кошелька не
//!    мешает, кошелёк не сверялся или его балансы не получены — круг не чистый;
//! 4) «Снять паузу» кошелька (Core::resume_wallet); из Telegram (меню «Площадки и карантин», «Состояние»): строка
//!    кошелька и кнопка «c:wr.<id>» → вопрос «Снять паузу кошелька …?» → та же команда ядра (ManualAction::ResumeWallet,
//!    Core::command с источником Telegram и «кто»), журнал core.command с нажавшим; повтор — «не на паузе»;
//! 5) чистая логика: тексты по кошелькам, «чистый круг» кошелька, короткое сообщение о паузе (paused_text), проблемы
//!    кошельков не входят в паузу площадки.
//!    Пример расхождения — случай 08.11: «PORTO по учёту 119.25, на площадке 0».
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (reconcile_wallet, reconcile_pause, reconcile::Found, risk, wallet_pause, dex_account),
//! settings (accounts), storage, notify (menu, menu_press).

use std::collections::HashSet;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::reconcile::Found;
use arb::core::reconcile_confirm::UNAVAILABLE;
use arb::core::reconcile_pause::{self, Lift};
use arb::core::reconcile_wallet::{is_clean, maybe_lift, mismatch_by_wallet, pause, paused_text};
use arb::core::risk::{Mode, can_open};
use arb::core::types::{Asset, DexAccountId, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::accounts::ROW_DEX_DEX;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use serde_json::json;
use tokio::sync::broadcast;

const PORTO: &str = "PORTO (0xabc) по учёту 119.25, на площадке 0";

fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

/// Ядро: виды площадок, все обязательные money.* и risk.* (как tests/core_daily_venue.rs), confirm_rounds = 2;
/// кошельки w1 «Основной» (строка bingx), w2 «Второй» (строка mexc), w3 «Циклы» (строка «DEX ↔ DEX»); «В работе».
async fn start(name: &str) -> Core {
    let d = std::env::temp_dir().join(format!("arb-230-{name}-{}", std::process::id()));
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
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![
        (v("bingx"), VenueKind::Cex),
        (v("bsc_dex"), VenueKind::Dex),
        (v("mexc"), VenueKind::Cex),
    ];
    for (n, x) in [
        ("money.min_profit_usd", "0.1"),
        ("money.min_profit_pct", "0.1"),
        ("money.max_per_trade_usd", "50"),
        ("money.max_per_coin_usd", "100"),
        ("money.max_in_transit_usd", "200"),
        ("risk.max_daily_loss_usd", "15"),
        ("risk.min_bnb_balance", "0.01"),
    ] {
        settings
            .set(n, Scope::Global, Some(json!(x)), "тест")
            .await
            .unwrap();
    }
    for id in ["bingx", "bsc_dex", "mexc"] {
        let scope = Scope::Venue(id.into());
        settings
            .set("risk.max_errors_in_row", scope, Some(json!(3)), "тест")
            .await
            .unwrap();
    }
    settings
        .set(
            "ledger.reconcile_confirm_rounds",
            Scope::Global,
            Some(json!(2)),
            "тест",
        )
        .await
        .unwrap();
    let a = settings.accounts();
    for (id, name, row) in [
        ("w1", "Основной", "bingx"),
        ("w2", "Второй", "mexc"),
        ("w3", "Циклы", ROW_DEX_DEX),
    ] {
        let secret = format!("wallet.{id}.key");
        let queue = format!("wallet:{id}");
        a.add(id, name, &secret, &queue, "тест").await.unwrap();
        a.map_set(row, Some(id), "тест").await.unwrap();
    }
    core.set_mode(Mode::Running).await;
    core
}

fn dir_of(buy: &str, sell: &str) -> Direction {
    Direction {
        asset: Asset {
            ticker: "PORTO".into(),
            bsc_contract: None,
        },
        buy_on: v(buy),
        sell_on: v(sell),
    }
}

/// Расхождение монеты на кошельке DEX (подпись строки сверки — «bsc_dex «имя»», как reconcile_holders.rs).
fn wallet_coin(id: &str, name: &str) -> Found {
    let label = format!("bsc_dex «{name}»");
    (
        "risk.reconcile_mismatch",
        Some("bsc_dex".into()),
        format!("{label}: {PORTO}"),
        Some(format!("{label}/PORTO")),
        Some(id.into()),
    )
}

/// «Балансы кошелька не получены».
fn wallet_unavailable(id: &str, name: &str) -> Found {
    let label = format!("bsc_dex «{name}»");
    (
        "risk.reconcile_mismatch",
        Some("bsc_dex".into()),
        format!("{label}: балансы не получены — таймаут"),
        Some(format!("{label}{UNAVAILABLE}")),
        Some(id.into()),
    )
}

/// Расхождение на бирже.
fn venue_coin(venue: &str) -> Found {
    (
        "risk.reconcile_mismatch",
        Some(venue.into()),
        format!("{venue}: PORTO по учёту 119.25, на площадке 0"),
        Some(format!("{venue}/PORTO")),
        None,
    )
}

fn set(xs: &[&str]) -> HashSet<String> {
    xs.iter().map(|x| x.to_string()).collect()
}

#[tokio::test]
async fn wallet_mismatch_pauses_only_this_wallet() {
    let core = start("only").await;
    let found = [wallet_coin("w1", "Основной"), venue_coin("mexc")];
    reconcile_pause::pause(&core, &found).await;
    pause(&core, &found).await;
    let r = core.control();
    assert_eq!(r.mode, Mode::Running, "системный режим не трогается");
    assert!(
        !r.venue_paused.contains_key(&v("bsc_dex")),
        "площадка DEX не на паузе"
    );
    assert!(
        r.venue_paused.contains_key(&v("mexc")),
        "биржа — как раньше"
    );
    let w = &r.wallet_paused["w1"];
    assert_eq!(w.name, "Основной");
    assert_eq!(w.venue, "bsc_dex");
    assert!(w.why.contains("PORTO (0xabc) по учёту 119.25"), "{}", w.why);
    assert!(!r.wallet_paused.contains_key("w2"));
    // Направления кошелька w1 (BingX ↔ DEX) не начинаются, причина — с именем кошелька.
    let why = can_open(&core, &dir_of("bingx", "bsc_dex")).unwrap_err();
    assert!(
        why.contains("кошелёк «Основной»") && why.contains("на паузе"),
        "{why}"
    );
    assert!(can_open(&core, &dir_of("bsc_dex", "bingx")).is_err());
    // Кошелёк циклов и BingX ↔ MEXC (без кошелька) торгуют; MEXC — на паузе своей площадки.
    assert_eq!(can_open(&core, &dir_of("bsc_dex", "bsc_dex")), Ok(()));
    assert!(
        can_open(&core, &dir_of("mexc", "bsc_dex"))
            .unwrap_err()
            .contains("площадка mexc на паузе")
    );
    core.resume_venue(&v("mexc")).await;
    assert_eq!(can_open(&core, &dir_of("mexc", "bsc_dex")), Ok(()));
    // Пауза кошелька циклов — отказ только циклам.
    pause(&core, &[wallet_unavailable("w3", "Циклы")]).await;
    assert!(
        can_open(&core, &dir_of("bsc_dex", "bsc_dex"))
            .unwrap_err()
            .contains("кошелёк «Циклы»")
    );
    assert_eq!(can_open(&core, &dir_of("mexc", "bsc_dex")), Ok(()));
    // Повторная постановка не перезаписывает причину первой.
    pause(&core, &[wallet_unavailable("w1", "Основной")]).await;
    assert!(core.control().wallet_paused["w1"].why.contains("PORTO"));
}

#[tokio::test]
async fn wallet_pause_lifts_by_own_rounds() {
    let core = start("lift").await;
    pause(&core, &[wallet_coin("w1", "Основной")]).await;
    let mut lift = Lift::default();
    let checked = set(&["w1", "w2"]);
    // Круг 1: w1 чистый, у w2 проблема — чужая проблема не мешает.
    maybe_lift(&core, &mut lift, &checked, &[wallet_coin("w2", "Второй")]).await;
    assert_eq!(lift.clean("w1"), 1);
    // Круг 2: балансы w1 не получены — счёт сначала.
    maybe_lift(
        &core,
        &mut lift,
        &checked,
        &[wallet_unavailable("w1", "Основной")],
    )
    .await;
    assert_eq!(lift.clean("w1"), 0);
    // Кошелёк не сверялся (удалён из списка) — не чистый.
    maybe_lift(&core, &mut lift, &set(&["w2"]), &[]).await;
    assert_eq!(lift.clean("w1"), 0);
    // Проблема биржи — не проблема кошелька.
    maybe_lift(&core, &mut lift, &checked, &[venue_coin("mexc")]).await;
    assert!(core.control().wallet_paused.contains_key("w1"));
    maybe_lift(&core, &mut lift, &checked, &[]).await;
    assert!(
        core.control().wallet_paused.is_empty(),
        "два чистых подряд — снята"
    );
    assert_eq!(can_open(&core, &dir_of("bingx", "bsc_dex")), Ok(()));
}

#[tokio::test]
async fn manual_resume_wallet() {
    let core = start("manual").await;
    pause(&core, &[wallet_coin("w2", "Второй")]).await;
    assert!(!core.resume_wallet("w1").await, "паузы нет — ничего");
    assert!(core.resume_wallet("w2").await);
    assert!(core.control().wallet_paused.is_empty());
    assert_eq!(can_open(&core, &dir_of("mexc", "bsc_dex")), Ok(()));
    // Счётчик кругов снятого вручную кошелька забывается.
    let mut lift = Lift::default();
    maybe_lift(&core, &mut lift, &set(&["w2"]), &[]).await;
    assert_eq!(lift.clean("w2"), 0);
}

/// Решение 230 (дополнение): пауза кошелька видна и снимается в Telegram — как пауза площадки.
#[tokio::test]
async fn resume_wallet_from_telegram_menu() {
    use arb::core::types::{CommandSource, ManualAction, ManualCommand};
    let core = start("tg").await;
    let settings = &core.inner.deps.settings;
    settings
        .set(
            "notify.telegram_chat",
            Scope::Global,
            Some(json!("100")),
            "тест",
        )
        .await
        .unwrap();
    pause(&core, &[wallet_coin("w2", "Второй")]).await;
    let r = core.control();
    let line =
        format!("Кошелёк «Второй» (DEX) — на паузе: сверка балансов: bsc_dex «Второй»: {PORTO}");
    let (text, kb) = arb::notify::menu::venues(&r);
    assert!(text.contains(&line), "{text}");
    let first = &kb["inline_keyboard"][0][0];
    assert_eq!(first["text"], "Снять паузу кошелька «Второй»");
    assert_eq!(first["callback_data"], "c:wr.w2");
    assert!(arb::notify::menu::state(&r, 0).0.contains(&line));
    let p = arb::notify::menu_press::control_action(&r, "wr.w2").unwrap();
    let action = ManualAction::ResumeWallet {
        wallet: "w2".into(),
    };
    assert_eq!(p.action, action);
    assert_eq!(p.question, "Снять паузу кошелька «Второй» (DEX)?");
    let cmd = ManualCommand {
        action,
        source: CommandSource::Telegram { chat: "100".into() },
        by: Some("@tigran (1)".into()),
        at: chrono::Utc::now(),
    };
    let t = core.command(cmd.clone()).await.unwrap();
    assert_eq!(t, "пауза кошелька «Второй» снята");
    assert!(core.control().wallet_paused.is_empty());
    assert_eq!(can_open(&core, &dir_of("mexc", "bsc_dex")), Ok(()));
    let (text, kb) = arb::notify::menu::venues(&core.control());
    assert!(text.contains("Площадок и кошельков на паузе, монет в карантине нет."));
    assert_eq!(
        kb["inline_keyboard"].as_array().unwrap().len(),
        1,
        "только «Назад»"
    );
    let t = core.command(cmd).await.unwrap();
    assert!(t.contains("не на паузе"), "{t}");
    // Кто снял — в журнале core.command (как у площадки); снятие — core.control с именем кошелька.
    let log = core.inner.deps.journal.list(200, None).await.unwrap();
    assert!(
        log.iter().any(|e| e.kind == "core.command"
            && e.message.contains("ResumeWallet")
            && e.data.as_ref().is_some_and(|d| d["by"] == "@tigran (1)")),
        "{log:?}"
    );
    assert!(
        log.iter().any(|e| e.kind == "core.control"
            && e.message.contains("Пауза кошелёк «Второй» снята вручную")),
        "{log:?}"
    );
}

#[tokio::test]
async fn wallet_of_direction_by_mapping() {
    let core = start("map").await;
    let w = |b: &str, s: &str| arb::core::dex_account::of_direction_now(&core, &dir_of(b, s));
    assert_eq!(w("bingx", "bsc_dex"), Some(DexAccountId("w1".into())));
    assert_eq!(w("bsc_dex", "mexc"), Some(DexAccountId("w2".into())));
    assert_eq!(w("bsc_dex", "bsc_dex"), Some(DexAccountId("w3".into())));
    assert_eq!(w("bingx", "mexc"), None, "биржа ↔ биржа — без кошелька");
    assert_eq!(w("gate", "bsc_dex"), None, "площадки нет в справочнике");
}

#[test]
fn pure_wallet_logic() {
    let found = [
        wallet_coin("w1", "Основной"),
        wallet_unavailable("w1", "Основной"),
        venue_coin("mexc"),
    ];
    let m = mismatch_by_wallet(&found);
    assert_eq!(m.len(), 1);
    assert_eq!(m["w1"].0, "bsc_dex");
    assert_eq!(m["w1"].1.len(), 2);
    // Проблемы кошельков — не проблемы площадки DEX (её пауза и её чистый круг).
    let by_venue = reconcile_pause::mismatch_by_venue(&found);
    assert_eq!(by_venue.keys().collect::<Vec<_>>(), vec!["mexc"]);
    assert!(reconcile_pause::is_clean(
        "bsc_dex",
        &set(&["bsc_dex"]),
        &found
    ));
    let checked = set(&["w1", "w2"]);
    assert!(!is_clean("w1", &checked, &found));
    assert!(is_clean("w2", &checked, &found));
    assert!(!is_clean("w3", &checked, &[]), "не сверялся — не чистый");
    let bnb: Found = (
        "risk.low_bnb",
        Some("bsc_dex".into()),
        "мало BNB".into(),
        None,
        Some("w2".into()),
    );
    assert!(
        is_clean("w2", &checked, &[bnb]),
        "мало BNB — не расхождение"
    );
}

#[test]
fn short_pause_message() {
    assert_eq!(
        paused_text(
            "mexc",
            "mexc",
            &["mexc: PORTO по учёту 119.25, на площадке 0"]
        ),
        "⏸ Пауза mexc: расхождение сверки — PORTO по учёту 119.25, на площадке 0. Снимется сама, когда балансы сойдутся"
    );
    let t = paused_text(
        "кошелёк «Основной»",
        "bsc_dex «Основной»",
        &[
            "bsc_dex «Основной»: PORTO по учёту 1, на площадке 0",
            "bsc_dex «Основной»: ATOM по учёту 2, на площадке 1",
        ],
    );
    assert_eq!(
        t,
        "⏸ Пауза кошелёк «Основной»: расхождение сверки — PORTO по учёту 1, на площадке 0; ATOM по учёту 2, на \
         площадке 1. Снимется сама, когда балансы сойдутся"
    );
    // Заголовок события не добавляется: текст уже со значком (notify/human.rs has_head).
    assert!(arb::notify::human::has_head(&t));
}

#[test]
fn paused_event_in_catalog_and_routed_to_failures() {
    use arb::notify::roles::Role;
    use arb::notify::routes::{Tag, extra};
    assert!(arb::notify::catalog::find("risk.reconcile_paused").is_some());
    assert_eq!(
        extra("risk.reconcile_paused", Tag::Plain),
        &[Role::Failures]
    );
    assert!(arb::notify::heads::missing().is_empty());
}
