//! Файл: tests/notify_human.rs
//! Что проверяет: решение 164 — человеческие тексты алертов. Округление (USDT и проценты — 2 знака, меньше 0.01 —
//! 4 знака, количество монеты — без длинного хвоста), человеческая причина для известных кодов бирж и «Ответ биржи»
//! для неизвестных, формат закрытия сделки (Тигран 05.11), заголовок у каждого события каталога.
//! Данные: живые ответы бирж 04.11.2026 — тексты msg из алертов Тиграна (MEXC 16021 по сделке
//! 20261104173851521BABYDOGE — тот же текст в venues/mexc/deposit_hold.rs; Gate TOO_FAST — «Withdrawal frequency is
//! limited to 10s»), пропущенные через настоящие функции адаптеров (venues/*/errors.rs); числа — из тех же алертов.

use std::str::FromStr;

use rust_decimal::Decimal;

use arb::core::deal::{Deal, Step};
use arb::core::types::{Asset, Direction, VenueId};
use arb::notify::human::{self, Msg};
use arb::notify::reason;
use arb::venues::{gate, mexc};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[test]
fn numbers_for_eyes() {
    // «Предварительный убыток … -44.66622 USDT» → 2 знака.
    assert_eq!(human::money(d("-44.66622")), "−44.67");
    assert_eq!(human::usd(d("-44.66622")), "44.67 USDT");
    assert_eq!(human::signed_usd(d("0.13")), "+0.13 USDT");
    assert_eq!(human::signed_usd(d("-0.15")), "−0.15 USDT");
    // |факт| < 0.01 — 4 знака, чтобы не было «0.00».
    assert_eq!(human::signed_usd(d("0.004")), "+0.0040 USDT");
    assert_eq!(human::pct(d("1.23456")), "1.23%");
    // Количество: 44.979648147666099998382387087 → 44.98; 139708165163 — целиком; GT 0.00405228703363914373.
    assert_eq!(human::qty(d("44.979648147666099998382387087")), "44.98");
    assert_eq!(human::qty(d("139708165163")), "139708165163");
    assert_eq!(human::qty(d("0.00405228703363914373")), "0.004052");
    assert_eq!(
        human::addr("0xc748673057861a797275cd8a068abb95a902e8de"),
        "0xc748…e8de"
    );
    assert_eq!(human::venue("bsc_dex"), "DEX");
    assert_eq!(human::venue("mexc"), "MEXC");
}

#[test]
fn known_codes_are_human() {
    // MEXC 16021 — живой ответ 04.11 через адаптер MEXC.
    let raw = r#"{"code":16021,"msg":"Some pre-crediting assets are temporarily unavailable for withdrawal. Please reconfirm the quantity."}"#;
    let e = mexc::errors::from_code(
        16021,
        "Some pre-crediting assets are temporarily unavailable for withdrawal. Please reconfirm the quantity.",
        mexc::errors::Op::Withdraw,
        raw,
    );
    let r = reason::human(&format!("Halted: перегон не состоялся: {}", e.message));
    assert_eq!(
        r.text,
        "Перегон не состоялся — MEXC ещё не до конца зачислил депозит — вывод временно запрещён"
    );
    assert!(r.answer.is_none());
    assert!(!r.text.contains("Halted"));

    // Gate TOO_FAST — живой ответ 04.11 через адаптер Gate.
    let e = gate::errors::from_label(
        400,
        "TOO_FAST",
        "Withdrawal frequency is limited to 10s 37.59.21.151",
        gate::errors::Op::Withdraw,
        "",
    );
    // Решение 179: у вывода TOO_FAST — «временно нельзя вывести» (повтор вывода), у прочего — лимит запросов.
    assert_eq!(e.code, arb::gateway::errors::ErrorCode::WithdrawLocked);
    let other = gate::errors::from_label(400, "TOO_FAST", "x", gate::errors::Op::Other, "");
    assert_eq!(other.code, arb::gateway::errors::ErrorCode::RateLimited);
    let r = reason::human(&format!("перегон не состоялся: {}", e.message));
    assert!(
        r.text
            .starts_with("Перегон не состоялся — Gate: слишком частый вывод"),
        "{}",
        r.text
    );
    assert!(!r.text.contains("37.59.21.151") && !r.text.contains("HTTP 400"));

    // Gate ADDRESS_NOT_USED: формулировка уже с именем площадки — имя не повторяется.
    let e = gate::errors::from_label(
        400,
        "ADDRESS_NOT_USED",
        "address not used",
        gate::errors::Op::Withdraw,
        "",
    );
    let r = reason::human(&e.message);
    assert_eq!(
        r.text,
        "Gate не разрешает вывод на этот адрес — адрес не подтверждён в адресной книге Gate"
    );
}

#[test]
fn unknown_code_goes_to_answer_line() {
    let e = mexc::errors::from_code(99999, "Something new", mexc::errors::Op::Withdraw, "");
    let r = reason::human(&format!("вывод не состоялся: {}", e.message));
    assert_eq!(r.text, "Вывод не состоялся");
    assert_eq!(r.answer.as_deref(), Some("MEXC 99999: Something new"));
    let text = Msg::new(human::STOP, "Биржа отклонила вывод")
        .reason(&format!("вывод не состоялся: {}", e.message))
        .id("20261104173851521BABYDOGE")
        .render();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "⛔ Биржа отклонила вывод");
    assert_eq!(lines[1], "Причина: Вывод не состоялся");
    assert_eq!(lines[2], "Ответ биржи: MEXC 99999: Something new");
    assert_eq!(lines.last(), Some(&"id 20261104173851521BABYDOGE"));
    // Номера решений человеку не нужны.
    assert_eq!(
        reason::human("Предварительный убыток в итог дня (решение 53)").text,
        "Предварительный убыток в итог дня"
    );
}

/// Сделка BABYDOGE mexc → DEX с заданными суммами. Принимает: план, вложено, выручку, затраты.
fn deal(plan: &str, invested: &str, proceeds: &str, spent: &str) -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "BABYDOGE".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("mexc".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut x = Deal::new(
        "20261104173851521BABYDOGE".into(),
        dir,
        serde_json::json!({}),
        d(plan),
    );
    x.invested = d(invested);
    x.proceeds = d(proceeds);
    x.spent = d(spent);
    x.step = Step::Closed;
    x.result_usd = Some(x.proceeds - x.invested - x.spent);
    x
}

#[test]
fn deal_closed_format() {
    let plus = arb::core::deal_text::closed(&deal("0.13", "45", "45.1", "0.01"), "продано");
    assert_eq!(
        plus,
        "✅ Сделка плюсовая\nBABYDOGE MEXC → DEX\nПлан: +0.13 USDT\nФакт: +0.09 USDT\nЗаработали: 0.09 USDT\n\nid 20261104173851521BABYDOGE"
    );
    let minus = arb::core::deal_text::closed(&deal("0.02", "45", "44.9", "0.05"), "продано");
    assert_eq!(
        minus,
        "❌ Сделка минусовая\nBABYDOGE MEXC → DEX\nПлан: +0.02 USDT\nФакт: −0.15 USDT\nИтого минус: 0.15 USDT\n\nid 20261104173851521BABYDOGE"
    );
    let tiny = arb::core::deal_text::closed(&deal("0.01", "45", "45.004", "0"), "продано");
    assert!(tiny.contains("Факт: +0.0040 USDT"), "{tiny}");
}

#[test]
fn every_event_has_head() {
    assert!(
        arb::notify::heads::missing().is_empty(),
        "{:?}",
        arb::notify::heads::missing()
    );
    for e in arb::notify::catalog::EVENTS {
        let h = arb::notify::heads::head(e.code, e.title);
        assert!(human::has_head(&h), "{}", e.code);
        assert!(!h.contains("(решени"), "{}: {h}", e.code);
    }
}
