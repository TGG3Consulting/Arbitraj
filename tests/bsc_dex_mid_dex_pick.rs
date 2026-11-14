//! Файл: tests/bsc_dex_mid_dex_pick.rs
//! Что делает: решение 276 (и дополнение — ревью) — выбор подписи промежуточной монеты без биржи (pools/mid_dex.rs
//! pick) на записи с цепи tests/data/fstswap_fist_2026-11-14/calls.json: symbol() FIST «FIST» и чужой обёртки fstswap
//! «WBNB». Проверки: подпись с цепи; совпадение с «BNB», тикером монеты подписки (без учёта регистра), монетой
//! venue.price_assets «GT» (решение 181) — адрес с причиной Taken; пустой и не-ABI ответ (32-байтное слово — ответ
//! decimals(), такой же формы, как bytes32 у старых токенов) — адрес; юникод-двойники (греческая «Ι», кириллические
//! «С»/«Т»), символы формата U+200B и U+202E, пробел внутри — адрес (BadChars); длиннее предела — адрес (TooLong);
//! предел не задан — адрес (NoMaxLen). Строки-двойники собраны из записанного «FIST» заменой одного символа.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::mid_dex, tests/mid_dex_common/rec.rs.
//! Предел 16 — значение теста (настройка dex.intermediate_label_max_len), не предложение.

use arb::venues::bsc_dex::pools::mid_dex::{Why, pick};

mod mid_dex_common;
use mid_dex_common::rec::{addr, rec, symbol};

const MAX: Option<u64> = Some(16);

fn busy(l: &[&str]) -> Vec<String> {
    l.iter().map(|s| s.to_string()).collect()
}

#[test]
fn pick_collisions() {
    let v = rec();
    let fist = addr(&v, "FIST");
    let wrapper = addr(&v, "WRAPPER");
    let s = symbol(&v, "FIST.symbol()_0x95d89b41");
    assert_eq!(s.as_deref(), Some("FIST"));
    assert_eq!(
        pick(s.clone(), fist, &busy(&["USDT", "BNB", "ETH"]), MAX),
        ("FIST".to_string(), None)
    );
    let as_addr = format!("{fist:#x}");
    assert_eq!(as_addr, "0xc9882def23bc42d53895b8361d0b1edc7570bc6a");
    // Совпало с тикером монеты подписки (без учёта регистра) — адрес.
    assert_eq!(
        pick(s, fist, &busy(&["fist"]), MAX),
        (as_addr.clone(), Some(Why::Taken("fist".into())))
    );
    // «BNB» — тикер газа: ни одна промежуточная его не получает.
    assert_eq!(
        pick(Some(" BNB ".into()), fist, &busy(&["BNB"]), MAX),
        (as_addr.clone(), Some(Why::Taken("BNB".into())))
    );
    // Монета venue.price_assets (Gate: GT, решение 181) занята так же.
    assert_eq!(
        pick(Some("gt".into()), fist, &busy(&["GT"]), MAX),
        (as_addr.clone(), Some(Why::Taken("GT".into())))
    );
    // Пусто и не-ABI ответ (32-байтное слово decimals() — не строка ABI) — адрес.
    assert_eq!(
        pick(Some("  ".into()), fist, &[], MAX),
        (as_addr.clone(), Some(Why::Empty))
    );
    let word = symbol(&v, "FIST.decimals()_0x313ce567");
    assert_eq!(word, None);
    assert_eq!(
        pick(word, fist, &[], MAX),
        (as_addr.clone(), Some(Why::NotAbi))
    );
    // Чужая обёртка fstswap: symbol() «WBNB»; занят — адрес, свободен — «WBNB» (но не «BNB»: цены 1:1 нет).
    let w = symbol(&v, "WRAPPER.symbol()_0x95d89b41");
    assert_eq!(w.as_deref(), Some("WBNB"));
    assert_eq!(
        pick(w.clone(), wrapper, &busy(&["WBNB", "BNB"]), MAX).0,
        format!("{wrapper:#x}")
    );
    assert_eq!(pick(w, wrapper, &busy(&["BNB"]), MAX).0, "WBNB");
}

#[test]
fn pick_unsafe_strings() {
    let v = rec();
    let fist = addr(&v, "FIST");
    let as_addr = format!("{fist:#x}");
    let s = symbol(&v, "FIST.symbol()_0x95d89b41").unwrap();
    let taken = busy(&["USDT", "BNB", "ETH"]);
    let bad = |x: String| pick(Some(x), fist, &taken, MAX);
    // Юникод-двойники: «FIST» с греческой Ι (U+0399), кириллическими С (U+0421) и Т (U+0422).
    for x in [
        s.replace('I', "\u{0399}"),
        s.replace('S', "\u{0421}"),
        s.replace('T', "\u{0422}"),
        // Двойник занятого «USDT» — кириллическая «Т»: совпадения по строке нет, отсекает набор символов.
        "USD\u{0422}".to_string(),
    ] {
        assert_eq!(bad(x), (as_addr.clone(), Some(Why::BadChars)));
    }
    // Символы формата: пробел нулевой ширины U+200B и смена направления письма U+202E; пробел внутри.
    for x in [
        "FI\u{200B}ST".to_string(),
        format!("{s}\u{200B}"),
        format!("\u{202E}{s}"),
        "FI ST".to_string(),
    ] {
        assert_eq!(bad(x), (as_addr.clone(), Some(Why::BadChars)));
    }
    // Допустимые знаки «.», «_», «-» и цифры — подпись с цепи.
    assert_eq!(bad("FIST.v2_b-1".into()), ("FIST.v2_b-1".to_string(), None));
    // Длиннее предела — адрес; ровно предел — подпись.
    let long = s.repeat(5);
    assert_eq!(bad(long), (as_addr.clone(), Some(Why::TooLong(16))));
    let exact = s.repeat(4);
    assert_eq!(bad(exact.clone()), (exact, None));
    // Предел не задан — подпись всегда адрес (зашитого предела нет).
    assert_eq!(
        pick(Some(s), fist, &taken, None),
        (as_addr, Some(Why::NoMaxLen))
    );
}
