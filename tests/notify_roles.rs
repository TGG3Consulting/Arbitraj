//! Файл: tests/notify_roles.rs
//! Что делает: проверяет решение 176 (Telegram — четыре чата) без обращения к Telegram: маршрутизацию исходящих событий
//! по ролям (закрытая плюсовая → «Сделки», минусовая → «Неудачные», остальное — только «Общий»); входящие — чат → роль,
//! чужой чат и чужая кнопка — отказ, разрешённые пользователи; без ролей — всё как раньше («Общий», зеркало туда же);
//! на настоящем ядре (SQLite во временной папке, без площадок): команда из чата «Управление» принимается, из чужого —
//! нет; «Перевести USDT» идёт командой ядра TransferUsdt (без площадки — отказ проверки, деньги не двигаются).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::notify, arb::settings, arb::storage. Моков нет. Записанных ответов Telegram
//! (getUpdates с callback_query) в репозитории нет — разбор ответа Telegram здесь не проверяется.

use std::path::PathBuf;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deal_text::outcome;
use arb::core::deps::Deps;
use arb::core::types::{Asset, CommandSource, Direction, ManualAction, ManualCommand, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::notify::press::accepts;
use arb::notify::roles::{self, Role};
use arb::notify::routes::{Tag, extra};
use arb::notify::tg_buttons::{Chat, User};
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

#[test]
fn outgoing_routes_by_role() {
    assert_eq!(extra("deal.closed", Tag::Profit), &[Role::Deals]);
    assert_eq!(extra("deal.closed", Tag::Loss), &[Role::Failures]);
    assert!(extra("deal.closed", Tag::Plain).is_empty());
    assert!(extra("usdt.relayed", Tag::Plain).is_empty());
    assert!(extra("manual.record", Tag::Plain).is_empty());
}

/// Сделка CAKE bingx → bsc_dex в шаге и с итогом.
fn deal(step: Step, result: Option<i64>) -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut d = Deal::new("t1CAKE".into(), dir, json!(null), Decimal::ONE);
    d.step = step;
    d.result_usd = result.map(Decimal::from);
    d
}

#[test]
fn closed_deal_outcome_picks_chat() {
    assert_eq!(outcome(&deal(Step::Closed, Some(0))), Some(true));
    assert_eq!(outcome(&deal(Step::Closed, Some(3))), Some(true));
    assert_eq!(outcome(&deal(Step::Closed, Some(-1))), Some(false));
    // Остановленная — не в «Сделки»: её запись с кнопками уходит в «Неудачные» зеркалом.
    assert_eq!(outcome(&deal(Step::Halted, Some(-1))), None);
}

#[test]
fn buttons_only_from_own_chat() {
    for c in ["k", "s", "t", "c", "y", "n"] {
        assert!(accepts(Role::Control, c), "{c}");
    }
    for c in ["sell_now", "sold_man", "m"] {
        assert!(!accepts(Role::Control, c), "{c}");
    }
    for c in ["sell_now", "sold_man", "usdt_retry", "y", "n"] {
        assert!(accepts(Role::Failures, c), "{c}");
    }
    for c in ["k", "s", "t", "m", "c"] {
        assert!(!accepts(Role::Failures, c), "{c}");
    }
    // «Общий» — как раньше: меню /menu, плашка, записи, подтверждения; кнопок «Управления» нет.
    for c in ["m", "c", "y", "sell_now"] {
        assert!(accepts(Role::General, c), "{c}");
    }
    for c in ["k", "s", "t"] {
        assert!(!accepts(Role::General, c), "{c}");
    }
    assert!(!accepts(Role::Deals, "y"));
}

/// Хранилище настроек в новой папке. Возвращает: (папка, база, журнал, настройки, поток изменений).
async fn store(
    name: &str,
) -> (
    PathBuf,
    Db,
    Journal,
    Store,
    broadcast::Sender<arb::settings::live::Change>,
) {
    let d: PathBuf = std::env::temp_dir().join(format!("arb-roles-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    (d, db, journal, settings, changes)
}

/// Задаёт настройку всей системы.
async fn set(s: &Store, name: &str, v: serde_json::Value) {
    s.set(name, Scope::Global, Some(v), "тест").await.unwrap();
}

fn chat(id: i64) -> Chat {
    Chat { id, username: None }
}

fn user(id: i64, name: &str) -> User {
    User {
        id,
        first_name: name.into(),
        username: Some(name.into()),
    }
}

#[tokio::test]
async fn incoming_chat_to_role_and_users() {
    let (_, _, _, s, _) = store("in").await;
    // Без ролей — как раньше: только «Общий», зеркало «Ручного управления» — туда же.
    set(&s, "notify.telegram_chat", json!("100")).await;
    assert_eq!(roles::configured(&s), vec![Role::General]);
    assert_eq!(roles::mirror_role(&s), Role::General);
    set(&s, "notify.control_chat", json!("-1002")).await;
    set(&s, "notify.failures_chat", json!("-1004")).await;
    assert_eq!(roles::mirror_role(&s), Role::Failures);
    let poll = [Role::Control, Role::General, Role::Failures];
    assert_eq!(roles::of_chat(&s, &chat(-1002), &poll), Some(Role::Control));
    assert_eq!(roles::of_chat(&s, &chat(100), &poll), Some(Role::General));
    assert_eq!(
        roles::of_chat(&s, &chat(-1004), &poll),
        Some(Role::Failures)
    );
    // Чужой чат — не наш; чат роли не из этого опроса (другой токен) — тоже не наш.
    assert_eq!(roles::of_chat(&s, &chat(555), &poll), None);
    assert_eq!(roles::of_chat(&s, &chat(-1002), &[Role::General]), None);
    // Пользователи не заданы — любой участник чата (как раньше); заданы — только они.
    assert!(roles::allowed(&s, Role::Control, &user(7, "anyone")));
    set(&s, "notify.control_users", json!(["@Tigran", "42"])).await;
    assert!(roles::allowed(&s, Role::Control, &user(1, "tigran")));
    assert!(roles::allowed(&s, Role::Control, &user(42, "x")));
    assert!(!roles::allowed(&s, Role::Control, &user(7, "anyone")));
    assert!(roles::allowed(&s, Role::General, &user(7, "anyone")));
    assert!(!roles::allowed(&s, Role::Deals, &user(1, "tigran")));
    // Ядро принимает команду только из чатов ролей.
    assert!(roles::is_role_chat(&s, "-1002"));
    assert!(!roles::is_role_chat(&s, "555"));
}

/// Ядро без площадок: чаты «Общий» 100 и «Управление» 200.
async fn core(name: &str) -> Core {
    let (d, db, journal, settings, changes) = store(name).await;
    set(&settings, "notify.telegram_chat", json!("100")).await;
    set(&settings, "notify.control_chat", json!("200")).await;
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("s.enc")),
        journal: journal.clone(),
    };
    let deps = Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d,
    };
    Core::start(deps).await
}

fn tg(chat: &str, action: ManualAction) -> ManualCommand {
    ManualCommand {
        action,
        source: CommandSource::Telegram { chat: chat.into() },
        by: Some("@tigran (1)".into()),
        at: Utc::now(),
    }
}

#[tokio::test]
async fn transfer_is_core_command_from_control_chat_only() {
    let c = core("tr").await;
    let a = ManualAction::TransferUsdt {
        from: "bingx".into(),
        from_wallet: None,
        to: "mexc".into(),
        to_wallet: None,
        amount: "10".into(),
    };
    assert_eq!(a.title(), "Перевести USDT");
    // Чужой чат — отказ до проверки перевода.
    let e = c.command(tg("300", a.clone())).await.unwrap_err();
    assert!(e.contains("не настроенный чат"), "{e}");
    // Чат «Управление» — команда дошла до перевода ядра: площадки нет — отказ проверки, перевода нет.
    let e = c.command(tg("200", a.clone())).await.unwrap_err();
    assert!(e.contains("площадки bingx нет"), "{e}");
    assert!(c.usdt_move("bingx").is_none());
    // Не число — отказ.
    let bad = ManualAction::TransferUsdt {
        from: "bingx".into(),
        from_wallet: None,
        to: "mexc".into(),
        to_wallet: None,
        amount: "десять".into(),
    };
    let e = c.command(tg("200", bad)).await.unwrap_err();
    assert!(e.contains("не число"), "{e}");
}
