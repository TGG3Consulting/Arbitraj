//! Файл: tests/notify_menu.rs
//! Что делает: проверяет команды и меню Telegram (решение 157) и общий путь команд ядра (решения 156, 157) без
//! обращения к Telegram: разбор «/cmd» и «/cmd@бот», /help со всеми командами, доступность «Старт» / «Пауза» / «Стоп» как
//! на плашке, вопросы подтверждения, ключ записи из старых сообщений; на настоящем ядре (SQLite во временной папке,
//! без площадок): остановленная сделка — запись экрана с кнопками (GET /api/manual, records), «Продано вручную» из
//! Telegram закрывает её, запись уходит с экрана, повтор команды не выполняет действие второй раз; «Старт» / «Стоп» и
//! «Сбросить лимит дня» из Telegram — те же команды ядра, что у плашки.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::notify, arb::settings, arb::storage. Моков нет.

use std::path::PathBuf;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::risk::{Mode, Risk};
use arb::core::types::{Asset, CommandSource, Direction, ManualAction, ManualCommand, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::notify::menu::{self, mode_disabled};
use arb::notify::menu_press::control_action;
use arb::notify::press_confirm::yes_title;
use arb::notify::press_record::record_key;
use arb::notify::tg_commands::{COMMANDS, help, parse};
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

#[test]
fn commands_parse_and_help() {
    assert_eq!(parse("/menu", "arb_bot"), Some("menu".into()));
    assert_eq!(
        parse("/Menu@ARB_bot лишнее", "arb_bot"),
        Some("menu".into())
    );
    assert_eq!(parse("/menu@other_bot", "arb_bot"), None);
    assert_eq!(parse("menu", "arb_bot"), None);
    assert_eq!(parse("/", "arb_bot"), None);
    let h = help();
    for (c, _) in COMMANDS {
        assert!(h.contains(&format!("/{c} — ")), "{c}");
        // BotCommand: 1-32 символа, строчные латинские, цифры, «_».
        assert!(
            c.len() <= 32
                && c.chars()
                    .all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_')
        );
    }
    for c in ["help", "menu", "dev", "start", "stop", "pause", "resume"] {
        assert!(COMMANDS.iter().any(|(x, _)| *x == c), "{c}");
    }
}

#[test]
fn mode_buttons_as_control_bar() {
    use Mode::*;
    // Как disabled у кнопок ControlBar.svelte.
    assert!(mode_disabled(Running, Running).is_some());
    assert!(mode_disabled(Running, Paused).is_none());
    assert!(mode_disabled(Running, Stopped).is_none());
    assert!(mode_disabled(Stopped, Paused).is_some());
    assert!(mode_disabled(Stopped, Stopped).is_some());
    assert!(mode_disabled(Paused, Running).is_none());
    let r = Risk::default(); // после запуска — «Стоп»
    let (_, kb) = menu::mode(&r);
    let data: Vec<&str> = kb["inline_keyboard"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x[0]["callback_data"].as_str().unwrap())
        .collect();
    assert_eq!(data, ["c:mode.Running", "m:root"]);
    assert!(control_action(&r, "mode.Paused").is_err());
    let p = control_action(&r, "mode.Running").unwrap();
    assert_eq!(p.action, ManualAction::SetMode { mode: Running });
    assert_eq!(yes_title(&p.title), "Да, старт");
    let p = control_action(&r, "dr.x").unwrap();
    assert_eq!(p.action, ManualAction::ResetDailyLimit);
    assert!(p.question.starts_with("Сбросить лимит дня — вся система"));
    // Решение 173: сброс лимита дня одной площадки.
    let p = control_action(&r, "dv.mexc").unwrap();
    assert_eq!(
        p.action,
        ManualAction::ResetVenueLimit {
            venue: "mexc".into()
        }
    );
    assert!(p.question.starts_with("Сбросить лимит дня площадки MEXC?"));
    assert_eq!(
        control_action(&r, "vr.bingx").unwrap().action,
        ManualAction::ResumeVenue {
            venue: "bingx".into()
        }
    );
}

#[test]
fn legacy_callback_is_alert() {
    assert_eq!(
        record_key("0b6f3c2e-1d4a-4f7e-9a51-3c8d2e7f1a90"),
        "a.0b6f3c2e-1d4a-4f7e-9a51-3c8d2e7f1a90"
    );
    assert_eq!(record_key("d.20261104CAKE"), "d.20261104CAKE");
}

/// Ядро на базе в новой папке; чат уведомлений — «100».
async fn start(name: &str) -> Core {
    let d: PathBuf = std::env::temp_dir().join(format!("arb-menu-{name}-{}", std::process::id()));
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
            "notify.telegram_chat",
            Scope::Global,
            Some(json!("100")),
            "тест",
        )
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("s.enc")),
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

/// Команда из Telegram от тестового пользователя.
fn tg(action: ManualAction) -> ManualCommand {
    ManualCommand {
        action,
        source: CommandSource::Telegram { chat: "100".into() },
        by: Some("@tigran (1)".into()),
        at: Utc::now(),
    }
}

#[tokio::test]
async fn halted_record_sold_manually_from_telegram_once() {
    let core = start("sold").await;
    let dir = Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut deal = Deal::new("t1CAKE".into(), dir, json!(null), Decimal::ONE);
    deal.step = Step::Halted;
    deal.at = VenueId("bsc_dex".into());
    deal.qty = Decimal::TEN;
    deal.invested = Decimal::from(25);
    arb::core::ledger::save(&core, &deal).await;
    let recs = core.manual_records();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].key, "d.t1CAKE");
    // Экран получает те же записи с кнопками.
    let screen = core.manual();
    assert_eq!(screen["records"][0]["key"], "d.t1CAKE");
    assert_eq!(screen["records"][0]["buttons"].as_array().unwrap().len(), 3);
    let b = recs[0].button("sold_man").unwrap();
    let ok = core
        .command(tg(b.action_with(Some("24.44"))))
        .await
        .unwrap();
    assert!(ok.contains("закрыта"), "{ok}");
    assert!(core.manual_records().is_empty());
    // Повтор того же действия — отказ, второй раз сделка не закрывается.
    let again = core
        .command(tg(b.action_with(Some("24.44"))))
        .await
        .unwrap_err();
    assert!(
        again.contains("закрыта") || again.contains("ведёт бот"),
        "{again}"
    );
}

#[tokio::test]
async fn control_commands_from_telegram() {
    let core = start("ctl").await;
    let t = core
        .command(tg(ManualAction::SetMode { mode: Mode::Paused }))
        .await
        .unwrap();
    assert!(t.contains("Пауза"), "{t}");
    assert_eq!(core.control().mode, Mode::Paused);
    assert!(core.control().reason.unwrap().contains("@tigran"));
    core.command(tg(ManualAction::SetMode {
        mode: Mode::Stopped,
    }))
    .await
    .unwrap();
    assert_eq!(core.control().mode, Mode::Stopped);
    let t = core
        .command(tg(ManualAction::ResumeVenue {
            venue: "bingx".into(),
        }))
        .await
        .unwrap();
    assert!(t.contains("не на паузе"), "{t}");
    assert!(
        core.command(tg(ManualAction::ResetDailyLimit))
            .await
            .is_ok()
    );
    // Чужой чат — отказ, как у кнопок.
    let mut c = tg(ManualAction::SetMode {
        mode: Mode::Running,
    });
    c.source = CommandSource::Telegram { chat: "200".into() };
    assert!(core.command(c).await.is_err());
    assert_eq!(core.control().mode, Mode::Stopped);
}

/// «Площадки и карантин» (notify/menu_venues.rs) и «Состояние»: пауза площадки подписана человеческим именем
/// (notify/human.rs venue(): bsc_dex → «DEX»), как строка паузы кошелька; callback_data — id площадки.
#[tokio::test]
async fn venue_pause_human_name() {
    let core = start("vname").await;
    let mut r = core.control();
    r.venue_paused
        .insert(VenueId("bsc_dex".into()), "исход неизвестен".into());
    let (text, kb) = menu::venues(&r);
    assert!(
        text.contains("Площадка DEX на паузе: исход неизвестен"),
        "{text}"
    );
    assert!(!text.contains("bsc_dex"), "{text}");
    let first = &kb["inline_keyboard"][0][0];
    assert_eq!(first["text"], "Снять паузу DEX");
    assert_eq!(first["callback_data"], "c:vr.bsc_dex");
    let (state, _) = menu::state(&r, 0);
    assert!(
        state.contains("Площадка DEX на паузе: исход неизвестен"),
        "{state}"
    );
    // Те же текст и кнопки — в «Состоянии» чата «Управление» (notify/ctl/screens.rs через menu::venue_buttons).
    let (t2, rows) = menu::venue_buttons(&r);
    assert_eq!(text, t2);
    assert_eq!(rows[0]["callback_data"], "c:vr.bsc_dex");
    // Вопрос подтверждения «Снять паузу площадки …?» (notify/menu_press.rs) — тем же именем, команда — по id.
    let p = control_action(&r, "vr.bsc_dex").unwrap();
    assert_eq!(p.question, "Снять паузу площадки DEX?");
    assert_eq!(
        p.action,
        ManualAction::ResumeVenue {
            venue: "bsc_dex".into()
        }
    );
}
