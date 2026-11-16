//! Файл: tests/bsc_dex_unread_and_breaks_339.rs
//! Что делает: DECISIONS «339 (экран и журнал, t153)» на записях живого бота 16.11.2026.
//! Проверяется:
//! (1) пулы, ни разу не прочитанные (pools/pool_unread.rs): отметка по найденному пулу (монета, второй токен, вид,
//!   причина), новая причина заменяет старую, снятие (как при записи в реестр), чистка по найденным; адреса и причина —
//!   из записи tests/data/dodo_kjudge_2026-11-16_2 (пул DPP USDC/USDT 0x298b… — откат KJUDGE_ERROR);
//! (2) запись об обрыве пачки (pools/link.rs): вид причины без чисел и hex (две записи журнала KJUDGE_ERROR с разными
//!   данными отката — один вид); ограничение частоты: первая — всегда, без интервала — каждая, с интервалом — не
//!   чаще, пропущенные считаются и называются в следующей; разные пачки и разные причины — независимо; каталог
//!   log.pool_break_interval_s.
//! Чего здесь нет: сам экран (api/coins.rs with_depths) и сессия пачки — нужен узел WebSocket.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools (pool_unread, link, discover, families), arb::settings::catalog.
//! Настройки: нет (интервал в тесте — число для расписания).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use alloy_primitives::Address;
use serde_json::Value;

use arb::core::types::PoolRef;
use arb::settings::catalog;
use arb::settings::schema::Kind;
use arb::venues::bsc_dex::pools::discover::Found;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::link::{self, State};
use arb::venues::bsc_dex::pools::pool_unread::Book;

const KJ: &str = "tests/data/dodo_kjudge_2026-11-16_2/kjudge_journal_entries_24h.json";
/// Пул DPP USDC/USDT из записи (KJUDGE_ERROR, base = USDC, quote = USDT).
const POOL: &str = "0x298ba9cd3acd572dce608ec682501c899628f176";
const USDC: &str = "0x8ac76a51cc950d9822d68b83fe1ad97b32cd580d";
const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";

fn addr(s: &str) -> Address {
    s.parse().expect("адрес")
}

/// Текст узла из записи журнала: между «код 3: » и « — ».
fn node_text(m: &str) -> String {
    m.split("код 3: ")
        .nth(1)
        .and_then(|s| s.split(" — ").next())
        .expect("текст узла")
        .to_string()
}

#[test]
fn unread_book_marks_and_clears() {
    let fam = families::make("dodo-dpp-test", "dodo_dpp").expect("вид DODO DPP");
    let (usdc, usdt) = (addr(USDC), addr(USDT));
    let (token0, token1) = if usdc < usdt {
        (usdc, usdt)
    } else {
        (usdt, usdc)
    };
    let r = PoolRef::Contract(addr(POOL));
    let f = Found {
        family: fam,
        reference: r.clone(),
        token0,
        token1,
        coin: usdc,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    };
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(KJ).expect("запись")).expect("JSON");
    let why = node_text(v[0]["message"].as_str().expect("message"));
    let b = Book::default();
    assert!(b.list().is_empty());
    b.note(&f, "узел не ответил");
    b.note(&f, &why);
    let list = b.list();
    assert_eq!(list.len(), 1, "одна отметка на пул");
    let (got, u) = &list[0];
    assert_eq!(got, &r);
    assert_eq!((u.coin, u.other), (usdc, usdt), "монета и второй токен");
    assert!(
        u.why.contains("KJUDGE_ERROR"),
        "последняя причина: {}",
        u.why
    );
    assert_eq!(u.family.id, "dodo-dpp-test");
    // Чистка по найденным: пул среди найденных — остаётся; нет — снимается.
    b.keep_only(&HashSet::from([r.clone()]));
    assert_eq!(b.list().len(), 1);
    b.clear(&r);
    assert!(b.list().is_empty(), "записан в реестр — отметка снята");
    b.note(&f, &why);
    b.keep_only(&HashSet::new());
    assert!(
        b.list().is_empty(),
        "пула нет среди найденных — отметка снята"
    );
}

#[test]
fn break_records_rate_limited_by_shape() {
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(KJ).expect("запись")).expect("JSON");
    let list = v.as_array().expect("массив");
    // Текст отката из записи: данные отката (hex) в виде причины не участвуют.
    let a = node_text(list[0]["message"].as_str().expect("message"));
    assert!(!link::shape(&a).contains("08c379a0"), "hex убран");
    let other_data = a.replace("08c379a0", "08c379a1");
    assert_ne!(a, other_data);
    assert_eq!(
        link::shape(&a),
        link::shape(&other_data),
        "другие данные — тот же вид"
    );
    let reason = link::shape("лог без позиции в блоке 133995628 0xdeadbeef");
    assert_eq!(reason, "лог без позиции в блоке  0x…");

    let mut s = State::default();
    let t0 = Instant::now();
    let key = |pack: &str| (pack.to_string(), link::shape(&a));
    // Без интервала — каждая запись.
    assert_eq!(link::due(&mut s, key("p1"), None, t0), (true, 0));
    assert_eq!(link::due(&mut s, key("p1"), None, t0), (true, 0));
    // С интервалом 60 с: внутри — пропуск со счётом; после — запись с числом пропущенных.
    let every = Some(Duration::from_secs(60));
    assert_eq!(
        link::due(&mut s, key("p2"), every, t0),
        (true, 0),
        "первая — всегда"
    );
    assert_eq!(
        link::due(&mut s, key("p2"), every, t0 + Duration::from_secs(10)),
        (false, 0)
    );
    assert_eq!(
        link::due(&mut s, key("p2"), every, t0 + Duration::from_secs(20)),
        (false, 0)
    );
    assert_eq!(
        link::due(&mut s, key("p2"), every, t0 + Duration::from_secs(61)),
        (true, 2),
        "пропущено два"
    );
    // Другая пачка и другая причина — независимо.
    assert_eq!(
        link::due(&mut s, key("p3"), every, t0 + Duration::from_secs(62)),
        (true, 0)
    );
    let other = ("p2".to_string(), link::shape("лента остановлена"));
    assert_eq!(
        link::due(&mut s, other, every, t0 + Duration::from_secs(63)),
        (true, 0)
    );
}

#[test]
fn break_interval_in_catalog() {
    let d = catalog::find(link::INTERVAL).expect("настройка в каталоге");
    assert!(matches!(d.kind, Kind::Integer));
}
