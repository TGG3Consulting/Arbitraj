//! Файл: tests/core_signal_max.rs
//! Что делает: карточка сигнала (решение 74) на записанных данных CAKE (tests/size_common: стакан BingX
//! ws_spot_2026-10-23, пул CAKE/USDT V2 на блоке 0x7b936ae): максимальный объём без бюджета (core/size_max.rs) —
//! прибыль на нём проходит порог, на шаг больше — нет или объём невозможен, оптимум не хуже перебора; край поиска —
//! загруженные уровни стакана (20) или предел card.max_search_usdt; ликвидность стакана в полосе (card_book.rs) —
//! сверка с суммой уровней вручную; подстановка шаблонов ссылок (card_links.rs).
//! Порог прибыли теста — прибыль на наибольшем объёме бюджета 50 USDT (заведомо проходящий объём есть); предел поиска
//! 1 000 000 USDT (точная граница — за size.bisect_steps = 32 шага).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{size_max, size_search, card_book, card_links}, tests/size_common.

mod size_common;

use arb::core::card_book::{in_band, side};
use arb::core::card_links::fill;
use arb::core::size::Curve;
use arb::core::size_max::max_volume;
use arb::core::size_search::search;
use arb::core::types::Side;
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{Rec, d, params};

/// Кривая с допуском 1 %.
fn curve(rec: &Rec, dir: Dir) -> RecCurve<'_> {
    RecCurve {
        rec,
        dir,
        slip: Decimal::ONE,
    }
}

#[tokio::test]
async fn max_volume_on_recorded_book_and_pool() {
    let rec = Rec::load("signal-max").await;
    for dir in [Dir::BingxToDex, Dir::DexToBingx] {
        let small = search(&mut curve(&rec, dir), &params(&rec, dir, d("50")))
            .await
            .unwrap();
        // Порог — прибыль на наибольшем объёме бюджета 50: он заведомо проходит, оптимум — тоже.
        let min = small.choice.at_budget_profit;
        let cap = d("1000000");
        let p = params(&rec, dir, cap);
        let mut c = curve(&rec, dir);
        let m = max_volume(&mut c, &p, small.choice.max_qty, |pt| pt.profit >= min)
            .await
            .unwrap();
        let q = m.max_qty.expect("заведомо проходящий объём есть");
        assert!(m.max_profit.unwrap() >= min);
        assert!(q >= small.choice.max_qty, "{dir:?}: {q}");
        assert!(
            !m.cap_reached,
            "{dir:?}: край — площадки, а не предел: {}",
            m.limit
        );
        // На шаг больше — порог не проходит или объём невозможен.
        let next = c.at(q + p.step).await;
        assert!(next.map(|pt| pt.profit < min).unwrap_or(true), "{dir:?}");
        // Оптимум без бюджета не хуже перебора целых монет до наибольшего объёма.
        let mut x = Decimal::ONE;
        while x <= m.upper_qty {
            if let Ok(pt) = c.at(x).await {
                assert!(
                    m.opt_profit >= pt.profit,
                    "{dir:?} {x}: {} > {}",
                    pt.profit,
                    m.opt_profit
                );
            }
            x += Decimal::ONE;
        }
        println!("{dir:?}: {m:?}");
    }
}

#[tokio::test]
async fn cap_limits_search() {
    let rec = Rec::load("signal-cap").await;
    let dir = Dir::BingxToDex;
    let p = params(&rec, dir, d("100"));
    let m = max_volume(&mut curve(&rec, dir), &p, Decimal::ZERO, |_| true)
        .await
        .unwrap();
    assert!(m.cap_reached, "{}", m.limit);
    assert!(m.limit.contains("card.max_search_usdt"));
    assert_eq!(
        m.max_qty,
        Some(m.upper_qty),
        "всё проходит — наибольший объём и есть край"
    );
}

/// Ликвидность в полосе ±0,5 % на записанном стакане CAKE-USDT@depth20 (последнее сообщение файла), сверка с ручным
/// подсчётом по уровням записи: asks от 2.189 до края 2.199945 — 10 уровней (2.189 … 2.198), 12102.078 CAKE на
/// 26551.233774 USDT; bids от 2.188 до края 2.17706 — 11 уровней, 17886.343 CAKE на 39016.840788 USDT. Полоса ±2 %
/// шире всех 20 загруженных уровней — отметка beyond.
#[tokio::test]
async fn book_liquidity_in_band() {
    let rec = Rec::load("signal-book").await;
    assert_eq!(
        (rec.asks[0].price, rec.bids[0].price),
        (d("2.189"), d("2.188"))
    );
    let (usd, qty, beyond) = in_band(&rec.asks, Side::Buy, d("0.5")).unwrap();
    assert_eq!(
        (usd, qty, beyond),
        (d("26551.233774"), d("12102.078"), false)
    );
    let (usd, qty, beyond) = in_band(&rec.bids, Side::Sell, d("0.5")).unwrap();
    assert_eq!(
        (usd, qty, beyond),
        (d("39016.840788"), d("17886.343"), false)
    );
    assert!(
        in_band(&rec.asks, Side::Buy, d("2")).unwrap().2,
        "все 20 уровней в полосе ±2 %"
    );
    // Не задана полоса — причина, а не ноль.
    let s = side(
        "bingx",
        Side::Buy,
        Some(&rec.asks),
        Err("не задана dex.depth_band_pct".into()),
    );
    assert!(s.usd_in_band.is_none() && s.error.is_some());
}

#[test]
fn link_templates() {
    let v = [("base", "CAKE"), ("quote", "USDT"), ("symbol", "CAKEUSDT")];
    assert_eq!(
        fill("https://bingx.com/en/spot/{symbol}", &v).unwrap(),
        "https://bingx.com/en/spot/CAKEUSDT"
    );
    assert_eq!(
        fill("https://x.example/{base}_{quote}/", &v).unwrap(),
        "https://x.example/CAKE_USDT/"
    );
    let pool = "0x3396021be1dd373ae9d69dd7270847f529432c6ca3ae39ab84b2a82b0b4e7a4e";
    assert_eq!(
        fill(
            "https://www.geckoterminal.com/bsc/pools/{pool}",
            &[("pool", pool)]
        )
        .unwrap(),
        format!("https://www.geckoterminal.com/bsc/pools/{pool}")
    );
    assert!(
        fill("https://a.example/{coin}", &v)
            .unwrap_err()
            .contains("{coin}")
    );
    assert!(fill("http://a.example/{base}", &v).is_err());
    assert!(fill("https://a.example/{base", &v).is_err());
    // Значение кодируется для адреса.
    assert_eq!(
        fill("https://a.example/{base}", &[("base", "A B")]).unwrap(),
        "https://a.example/A%20B"
    );
}
