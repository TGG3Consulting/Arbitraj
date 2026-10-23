//! Файл: tests/notify_buttons.rs
//! Что делает: проверяет логику кнопок Telegram без обращения к Telegram: упаковку и разбор callback_data
//! (предел протокола 64 байта), клавиатуру события из каталога, превращение кнопки в команду ядру,
//! проверку «нажатие из настроенного чата», предел длины текста.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::notify. Моков нет. Записанных ответов Telegram (getUpdates с callback_query)
//! в репозитории пока нет — разбор ответа Telegram здесь не проверяется (нужен токен и реальное нажатие).

use arb::core::types::ManualAction;
use arb::notify::callback::{self, CALLBACK_DATA_MAX_BYTES, CallbackError};
use arb::notify::catalog;
use arb::notify::press::chat_matches;
use arb::notify::tg_buttons::Chat;
use arb::notify::tg_text::fit_text;

#[test]
fn encode_decode_roundtrip() {
    let data = callback::encode("sell_now", "alert:42").unwrap();
    assert_eq!(data, "sell_now:alert:42");
    // Делим по первому «:» — id может содержать двоеточия.
    assert_eq!(callback::decode(&data), Some(("sell_now", "alert:42")));
    assert_eq!(callback::decode("sell_now"), None);
    assert_eq!(callback::decode(":x"), None);
    assert_eq!(callback::decode("x:"), None);
}

#[test]
fn encode_respects_protocol_limit() {
    let fits = "a".repeat(CALLBACK_DATA_MAX_BYTES - "take_over:".len());
    assert!(callback::encode("take_over", &fits).is_ok());
    let too_long = format!("{fits}b");
    assert!(matches!(
        callback::encode("take_over", &too_long),
        Err(CallbackError::TooLong {
            len: 65,
            max: 64,
            ..
        })
    ));
    // Предел — в байтах: кириллица по 2 байта.
    let cyr = "я".repeat(28);
    assert!(callback::encode("take_over", &cyr).is_err());
    assert_eq!(
        callback::encode("sell_now", ""),
        Err(CallbackError::EmptyObject("sell_now".into()))
    );
}

#[test]
fn keyboard_from_catalog() {
    let def = catalog::find("stuck.loss_alert").unwrap();
    let kb = callback::keyboard(def, "A1").unwrap().unwrap();
    let row = &kb["inline_keyboard"][0];
    assert_eq!(row.as_array().unwrap().len(), def.buttons.len());
    for (b, j) in def.buttons.iter().zip(row.as_array().unwrap()) {
        assert_eq!(j["text"], b.title);
        assert_eq!(j["callback_data"], format!("{}:A1", b.code));
    }
    let plain = catalog::find("system.started").unwrap();
    assert_eq!(callback::keyboard(plain, "A1").unwrap(), None);
}

#[test]
fn every_catalog_button_maps_to_command() {
    for e in catalog::EVENTS {
        for b in e.buttons {
            assert!(
                callback::to_action(b.code, "x").is_some(),
                "кнопка {} без команды ядру",
                b.code
            );
            assert_eq!(callback::title(b.code), Some(b.title));
        }
    }
    assert_eq!(
        callback::to_action("sell_now", "A1"),
        Some(ManualAction::SellNow {
            alert_id: "A1".into()
        })
    );
    assert_eq!(
        callback::to_action("take_over", "A1"),
        Some(ManualAction::TakeOver {
            alert_id: "A1".into()
        })
    );
    assert_eq!(callback::to_action("other", "A1"), None);
}

#[test]
fn only_configured_chat() {
    let chat = Chat {
        id: -1001234567890,
        username: Some("arb_alerts".into()),
    };
    assert!(chat_matches("-1001234567890", &chat));
    assert!(chat_matches("@arb_alerts", &chat));
    assert!(!chat_matches("-100123", &chat));
    assert!(!chat_matches("@other", &chat));
    assert!(!chat_matches("arb_alerts", &chat));
    let private = Chat {
        id: 42,
        username: None,
    };
    assert!(!chat_matches("@arb_alerts", &private));
}

#[test]
fn text_fits_protocol_limit() {
    assert_eq!(fit_text("коротко"), "коротко");
    let long = "😀".repeat(3000); // 6000 единиц UTF-16
    let t = fit_text(&long);
    assert!(t.encode_utf16().count() <= 4096);
    assert!(t.ends_with('…'));
}
