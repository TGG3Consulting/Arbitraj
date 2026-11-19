//! Файл: tests/notify_buttons_408.rs
//! Что делает: решение 408 (t193, Тигран 18.11: «обрабатывать тревоги прямо из Telegram») — проверки без обращения к
//! Telegram и без моков сети: кнопки тревог (notify/acts.rs) — тот же callback «c:…», что у меню /menu (control_action
//! принимает его), действия «e:…» — в пределе callback_data 1–64 байта
//! (https://core.telegram.org/bots/api#inlinekeyboardbutton) и разбираются notify/callback.rs; длинная кнопка — не в
//! клавиатуре, а строкой «только в интерфейсе» (и в меню — menu.rs b → «c:x»); новые команды ядра — надписи и разбор
//! JSON; у записи «своих монет не видно» — «Открыть сделку <ID>» сделок-держателей (данные записи — как
//! tests/notify_mirror.rs, сделка CAKE).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::notify (acts, callback, menu, menu_press, mirror_view), arb::core (types, risk, manual_rows).

use arb::core::manual_rows::alert_record;
use arb::core::risk::Risk;
use arb::core::types::{Asset, ManualAction, StuckAlert, VenueId};
use arb::notify::acts::{self, e};
use arb::notify::callback::{CALLBACK_DATA_MAX_BYTES, decode};
use arb::notify::menu;
use arb::notify::menu_press::control_action;
use arb::notify::mirror_view::record_message_for;
use chrono::Utc;
use rust_decimal::Decimal;

/// Адрес CAKE в BSC (как в записанных данных tests/size_common).
const CAKE: &str = "0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82";

#[test]
fn control_buttons_same_callback_as_menu() {
    let mut r = Risk::default();
    r.venue_paused
        .insert(VenueId("bingx".into()), "правило шлюза".into());
    for a in [
        acts::resume_venue("bingx"),
        acts::start(),
        acts::reset_daily(),
        acts::reset_venue_limit("bingx"),
        acts::release_quarantine(CAKE, "CAKE"),
        acts::resume_wallet("w1", "MEXC"),
    ] {
        let (code, object) = decode(&a.data).expect("формат «код:объект»");
        assert_eq!(code, "c", "{}", a.data);
        // Тот же разбор, что у кнопки меню: вопрос «Да, …» или причина недоступности (как у плашки).
        // После запуска режим «Стоп» — «Старт» доступен.
        let res = control_action(&r, object);
        assert!(res.is_ok(), "{}: {:?}", a.data, res.err());
    }
    assert_eq!(acts::resume_venue("bingx").title, "Снять паузу BingX");
}

#[test]
fn event_actions_fit_callback_limit() {
    let uuid = "3f2b8c1e-9a4d-4e6b-8f0a-1c2d3e4f5a6b";
    let deal = "20261104120000123CAKE";
    for a in [
        e::release_queue("bsc_dex", Some("w1"), "кошелька"),
        e::release_queue("bsc_dex", None, "кошелька"),
        e::cancel_wait(uuid, "Продать сейчас", "CAKE"),
        e::open_deal(deal),
        e::transfer(
            "bingx",
            "bsc_dex/w1",
            "123.45",
            "Перегнать 123.45 USDT снова",
        ),
        e::transfer_to("bsc_dex/w1", "Кошелёк DEX «MEXC»"),
        e::sell("bingx", "CAKE"),
        e::deploy_executor("bsc_dex", "w1", "MEXC"),
        e::open_entry("bingx", CAKE, "CAKE"),
        e::unblacklist(CAKE, "CAKE"),
    ] {
        assert!(
            !a.data.is_empty() && a.data.len() <= CALLBACK_DATA_MAX_BYTES,
            "{} — {} байт",
            a.data,
            a.data.len()
        );
        let (code, object) = decode(&a.data).unwrap();
        assert_eq!(code, "e");
        assert!(object.contains('.'), "{object}");
    }
    assert_eq!(
        e::release_queue("bsc_dex", None, "x").data,
        "e:qh.bsc_dex.-"
    );
    assert_eq!(e::open_deal(deal).data, format!("e:od.{deal}"));
}

#[test]
fn long_button_goes_to_text() {
    let long = "x".repeat(CALLBACK_DATA_MAX_BYTES);
    let (kb, notes) = acts::keyboard(&[acts::resume_venue("bingx"), e::open_deal(&long)]);
    let kb = kb.expect("короткая кнопка осталась");
    let rows = kb["inline_keyboard"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0]["callback_data"], "c:vr.bingx");
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("только в интерфейсе"), "{}", notes[0]);
    let (none, n2) = acts::keyboard(&[]);
    assert!(none.is_none() && n2.is_empty());

    // Меню /menu: id площадки длиннее предела — кнопка «… — только в интерфейсе» с «c:x», не отказ всего сообщения.
    let mut r = Risk::default();
    r.venue_paused.insert(VenueId(long.clone()), "пауза".into());
    let (_, kb) = menu::venues(&r);
    let b = &kb["inline_keyboard"][0][0];
    assert_eq!(b["callback_data"], "c:x");
    assert!(b["text"].as_str().unwrap().ends_with("только в интерфейсе"));
    let why = control_action(&r, "x").unwrap_err();
    assert!(why.contains("только в интерфейсе"), "{why}");
}

#[test]
fn new_commands_titles_and_json() {
    let a: ManualAction =
        serde_json::from_str(r#"{"ReleaseWalletQueue":{"venue":"bsc_dex"}}"#).unwrap();
    assert_eq!(
        a,
        ManualAction::ReleaseWalletQueue {
            venue: "bsc_dex".into(),
            wallet: None,
            pending_ok: false,
            nonce: None,
            hash: None,
        }
    );
    assert_eq!(a.title(), "Снять очередь кошелька");
    let b = ManualAction::OpenCoinEntry {
        coin: CAKE.into(),
        venue: "bingx".into(),
    };
    let back: ManualAction = serde_json::from_value(serde_json::to_value(&b).unwrap()).unwrap();
    assert_eq!(back, b);
    assert_eq!(b.title(), "Открыть вход монеты");
    assert_eq!(
        ManualAction::Unblacklist { coin: CAKE.into() }.title(),
        "Убрать из чёрного списка"
    );
    let d = ManualAction::DeployExecutor {
        venue: "bsc_dex".into(),
        wallet: "w1".into(),
        sha256: "ab".into(),
        wnative: "0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c".into(),
        executor: None,
    };
    assert_eq!(d.title(), "Развернуть исполнитель");
}

/// Алерт «своих монет не видно» сделки CAKE с держателями (как tests/notify_mirror.rs).
fn foreign(holders: Vec<String>) -> StuckAlert {
    StuckAlert {
        id: "a-408".into(),
        deal_id: "20261104120000123CAKE".into(),
        venue: VenueId("bingx".into()),
        asset: Asset {
            chain: None,
            ticker: "CAKE".into(),
            contract: None,
        },
        qty: Decimal::new(1234, 2),
        loss_pct: None,
        reason: Some("Своих монет на счёте не видно".into()),
        at: Utc::now(),
        error: None,
        attempts: 1,
        holders,
    }
}

#[test]
fn foreign_record_opens_holder_deals() {
    let ids = vec![
        "20261104115900001CAKE".to_string(),
        "20261104115800002CAKE".to_string(),
    ];
    let rec = alert_record(&foreign(ids.clone()), Decimal::ZERO);
    assert_eq!(rec.holders, ids);
    let (_, kb) = record_message_for(&rec, true);
    let kb = kb.unwrap();
    let all: Vec<String> = kb["inline_keyboard"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r.as_array().cloned().unwrap_or_default())
        .filter_map(|b| b["callback_data"].as_str().map(str::to_string))
        .collect();
    for id in &ids {
        assert!(all.contains(&format!("e:od.{id}")), "{all:?}");
    }
    // Обычный алерт — без «Открыть сделку».
    let (_, kb) = record_message_for(&alert_record(&foreign(vec![]), Decimal::ZERO), true);
    let s = kb.map(|k| k.to_string()).unwrap_or_default();
    assert!(!s.contains("e:od."), "{s}");
}

/// Ответ Тиграна 18.11: денежные кнопки тревог — не старше notify.dialog_ttl_s по времени сообщения тревоги.
#[test]
fn money_alert_buttons_expire() {
    use arb::notify::press_event_ttl::{MONEY, alert_time, money_action, within};
    use std::time::Duration;
    let ttl = Duration::from_secs(600);
    assert!(within(1_000, ttl, 1_600));
    assert!(!within(1_000, ttl, 1_601));
    for op in ["qh", "sb", "tu", "tt", "tf", "tx", "xd"] {
        assert!(MONEY.contains(&op), "{op}");
    }
    for op in ["wc", "od", "oe", "ub"] {
        assert!(!MONEY.contains(&op), "{op}");
    }
    // Ревью t193 М1: шаги перевода несут время тревоги — срок от неё, а не от шага.
    assert_eq!(
        alert_time("tx", "1000.bingx.mexc.12.5", 9_999),
        Some((1_000, "bingx.mexc.12.5"))
    );
    assert_eq!(
        alert_time("tf", "1000.bingx.mexc", 9_999),
        Some((1_000, "bingx.mexc"))
    );
    assert_eq!(
        alert_time("tu", "bingx.mexc.12.5", 9_999),
        Some((9_999, "bingx.mexc.12.5"))
    );
    assert_eq!(alert_time("tf", "x.bingx.mexc", 9_999), None);
    // Ревью t193 Н1: «Да» денежных команд — тоже со сроком.
    assert!(money_action(&ManualAction::SellBalance {
        venue: "bingx".into(),
        account: "Спот".into(),
        asset: "CAKE".into(),
    }));
    assert!(!money_action(&ManualAction::ResetDailyLimit));
}
