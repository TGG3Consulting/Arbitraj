//! Файл: core/feed_print_tests.rs
//! Что проверяет: решение 213, подтверждение тихой пары ПО СОДЕРЖИМОМУ стакана (Тигран 08.11; feed_print.rs и путь
//! через feed_confirm.rs verdict / apply): первые N уровней REST и стакана в памяти равны — тихая, подтверждена
//! (Confirmed, без переподписки); равны при разной записи десятичных («0.1230» = «0.123»); отличается объём или цена
//! одного уровня, число уровней или стакана в памяти нет — кадры потеряны (Lost + круг переподписки решения 202);
//! кадр пришёл, пока шёл запрос, — ответ не нужен; у пары и ответа есть номер — решает номер, как у Gate.
//! Без моков: вход — живые структуры стыка (gateway/feed_health.rs), числа настроек — советы docs/FIRST_RUN.md.
//! Записанных пар «кадр WebSocket + ответ REST» MEXC и Bitget в tests/data нет (tests/README.md) — разбор ответов
//! адаптеров проверяется в tests/mexc_real_data.rs и tests/bitget_real_data.rs.
//! Кто вызывает: cargo test.

use std::collections::HashMap;
use std::str::FromStr;

use rust_decimal::Decimal;

use super::feed_confirm::{QuietCfg, Verdict, apply, verdict};
use super::feed_watch_logic::{Action, Cfg, State, decide};
use crate::core::types::PriceLevel;
use crate::gateway::feed_health::{
    BookPrint, BookPrints, BookVersion, ConnHealth, FeedHealth, PairHealth,
};

fn cfg() -> Cfg {
    Cfg {
        book_stale_ms: 5_000,
        dead_mult: 3,
        live_ms: 40_000,
        warmup_ms: 30_000,
        attempts: 3,
        interval_ms: 3_000,
        pause_ms: 60_000,
        dead_pct: 30,
        reconnect_min_ms: 120_000,
        reconnect_max_per_hour: 3,
        quiet: QuietCfg::Confirm {
            min_ms: 30_000,
            max_per_s: 5,
        },
    }
}

/// Пара без номера книги (MEXC, Bitget: номер в учёт свежести не передаётся).
fn pair(last: Option<i64>) -> PairHealth {
    PairHealth {
        ticker: "CAKE".into(),
        last_ms: last,
        since_ms: 0,
        last_id: None,
        last_changed_ms: None,
    }
}

fn health(now: i64, last: Option<i64>) -> FeedHealth {
    FeedHealth {
        scheduled: false,
        per_pair: true,
        confirm: true,
        conns: vec![ConnHealth {
            conn: "market-1".into(),
            alive_ms: Some(now),
            started_ms: Some(0),
            pairs: vec![pair(last)],
        }],
    }
}

/// Уровни из строк «цена:объём».
fn lv(s: &[&str]) -> Vec<PriceLevel> {
    s.iter()
        .map(|x| {
            let (p, q) = x.split_once(':').unwrap();
            PriceLevel {
                price: Decimal::from_str(p).unwrap(),
                qty: Decimal::from_str(q).unwrap(),
            }
        })
        .collect()
}

fn print(bids: &[&str], asks: &[&str]) -> BookPrint {
    BookPrint::top(&lv(bids), &lv(asks), 5)
}

fn ver(rest: BookPrint, ours: Option<BookPrint>) -> BookVersion {
    BookVersion {
        print: Some(BookPrints {
            levels: 5,
            rest,
            ours,
        }),
        ..Default::default()
    }
}

#[test]
fn equal_content_quiet() {
    let p = pair(Some(10_000));
    let a = print(&["2.101:10", "2.100:5"], &["2.102:7", "2.103:1"]);
    assert_eq!(
        verdict(&p, &ver(a.clone(), Some(a)), 40_000),
        Verdict::Quiet
    );
    // Та же книга, другая запись десятичных (REST «2.1010», кадр «2.101») — точное равенство значений.
    let rest = print(&["2.1010:10.0", "2.100:5"], &["2.102:7", "2.10300:1"]);
    let ours = print(&["2.101:10", "2.1:5"], &["2.102:7.000", "2.103:1"]);
    assert_eq!(verdict(&p, &ver(rest, Some(ours)), 40_000), Verdict::Quiet);
    // Пустая книга с обеих сторон тоже равна.
    let e = print(&[], &[]);
    assert_eq!(
        verdict(&p, &ver(e.clone(), Some(e)), 40_000),
        Verdict::Quiet
    );
}

#[test]
fn different_content_lost() {
    let p = pair(Some(10_000));
    let ours = print(&["2.101:10", "2.100:5"], &["2.102:7"]);
    // Объём второго уровня покупателей другой — без допусков.
    let rest = print(&["2.101:10", "2.100:5.0001"], &["2.102:7"]);
    let v = verdict(&p, &ver(rest, Some(ours.clone())), 40_000);
    assert!(
        matches!(&v, Verdict::Lost(w) if w.contains("первые 5 уровней")),
        "{v:?}"
    );
    // Уровней разное число.
    let rest = print(&["2.101:10"], &["2.102:7"]);
    assert!(matches!(
        verdict(&p, &ver(rest, Some(ours)), 40_000),
        Verdict::Lost(_)
    ));
    // Стакана в памяти нет.
    let rest = print(&["2.101:10"], &["2.102:7"]);
    let v = verdict(&p, &ver(rest, None), 40_000);
    assert!(
        matches!(&v, Verdict::Lost(w) if w.contains("в памяти нет")),
        "{v:?}"
    );
}

#[test]
fn number_wins_when_both_have_it() {
    // У кадра и ответа есть номер (Gate) — решает номер, отпечатки не смотрятся.
    let mut p = pair(Some(10_000));
    p.last_id = Some(100);
    let rest = print(&["1:1"], &[]);
    let ours = print(&["1:2"], &[]);
    let v = BookVersion {
        id: Some(100),
        ..ver(rest, Some(ours))
    };
    assert_eq!(verdict(&p, &v, 40_000), Verdict::Quiet);
}

#[test]
fn apply_confirmed_and_lost_by_content() {
    let (c, none) = (cfg(), HashMap::new());
    let mut st = State::default();
    let check = Action::Check {
        conn: "market-1".into(),
        ticker: "CAKE".into(),
    };
    // Молчит 30 с после прогрева — проверка по REST.
    assert_eq!(
        decide(&health(40_000, Some(10_000)), 40_000, &c, &none, &mut st),
        vec![check]
    );
    let a = print(&["2.101:10"], &["2.102:7"]);
    let r = Ok(ver(a.clone(), Some(a.clone())));
    let out = apply(
        &health(40_100, Some(10_000)),
        "market-1",
        "CAKE",
        r,
        40_000,
        40_100,
        &c,
        &mut st,
    );
    assert_eq!(
        out,
        vec![Action::Confirmed {
            conn: "market-1".into(),
            ticker: "CAKE".into(),
            until_ms: 73_000,
            log: true,
        }]
    );
    // Через venue.quiet_confirm_min_ms книга на бирже другая — кадры потеряны, круг переподписки.
    assert_eq!(
        decide(&health(70_000, Some(10_000)), 70_000, &c, &none, &mut st).len(),
        1
    );
    let rest = print(&["2.100:3"], &["2.102:7"]);
    let out = apply(
        &health(70_100, Some(10_000)),
        "market-1",
        "CAKE",
        Ok(ver(rest, Some(a.clone()))),
        70_000,
        70_100,
        &c,
        &mut st,
    );
    assert!(
        matches!(&out[..], [Action::Lost { why, .. }, Action::Resub { attempt: 1, round: 1, .. }] if why.contains("2.100×3")),
        "{out:?}"
    );
}

#[test]
fn fresh_frame_during_request_ignores_answer() {
    let c = cfg();
    let mut st = State::default();
    // Кадр пришёл в 40 050, пока шёл запрос: отпечатки могли разойтись из-за гонки — ответ не нужен.
    let rest = print(&["1:1"], &[]);
    let ours = print(&["1:2"], &[]);
    let out = apply(
        &health(40_100, Some(40_050)),
        "market-1",
        "CAKE",
        Ok(ver(rest, Some(ours))),
        40_000,
        40_100,
        &c,
        &mut st,
    );
    assert!(out.is_empty(), "{out:?}");
}
