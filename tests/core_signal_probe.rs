//! Файл: tests/core_signal_probe.rs
//! Что делает: решение 260 — объём проверки сигнала signal.probe_usd (core/signal_probe.rs, core/prices.rs) на записанных
//! данных CAKE (tests/size_common: стакан BingX ws_spot_2026-10-23, правила пары и сети BingX 2026-10-24, пул CAKE/USDT
//! PancakeSwap V2 на блоке 0x7b936ae; обмен считает адаптер DEX quote/pool_quote.rs) и настоящей SQLite:
//! 1) настройка не задана — объём сравнения побайтно прежний (потолок / опорная цена), спред и состояние те же;
//! 2) задана — объём min(probe, потолок) / цена, не меньше минимумов площадок (minQty, minNotional / цена, withdrawMin,
//!    depositMin из записи), не больше потолка;
//! 3) чтение: не задана — None; вся система; площадка покупки A; пара «площадка + тип направления» сильнее;
//! 4) DEX → BingX на записи: на потолке 40 000 USDT спред отрицательный — «ниже порога», на объёме проверки 50 USDT —
//!    плюс и «кандидат»; спред на меньшем объёме не ниже, чем на большем (на всех точках записи);
//! 5) подбор объёма кандидата (size_search.rs) с оценкой старта бюджет / цена покупки сигнала (profit.rs: у DEX на A
//!    стакана нет — цена из строки сигнала) — на объёме проверки и на потолке: тот же наибольший объём (верхняя граница —
//!    бюджет = потолок), тот же выбор, расчётов не больше.
//!
//! Затрат сверх цены в записях нет (ставки taker аккаунта не записаны) — порог комиссий сигнала fees_pct = 0, как у
//! котировок записи; комиссия пула 0,25 % — внутри цены пула (dex.v2_fee_pct теста, tests/size_common).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{signal, signal_probe, size_search}, arb::settings, tests/size_common.

mod size_common;

use arb::core::signal::{Params, Prices, Signals, State};
use arb::core::signal_probe::{floor_qty, probe_usd, qty};
use arb::core::size_search::search;
use arb::core::types::{Asset, Direction, VenueId};
use arb::settings::read::At;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::settings::venue_dir::{DirKind, VenueDir};
use arb::storage::db::Db;
use arb::venues::bingx::book::walk;
use arb::venues::bsc_dex::num::{to_decimal, to_units};
use arb::venues::bsc_dex::quote::pool_quote::swap;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use size_common::curve::{Dir, RecCurve};
use size_common::{CAKE, Rec, USDT, d, params};
use tokio::sync::broadcast;

/// Цены DEX → BingX на qty монет по записи: покупка в пуле (точный выход, адаптер DEX), продажа по bids BingX.
/// fees_pct — 0 (затрат сверх цены в записи нет).
fn dex_to_bingx(rec: &Rec, q: Decimal) -> Prices {
    let b = swap(&rec.ctx, &rec.pool, USDT, false, to_units(q, 18).unwrap()).unwrap();
    let (filled, sum) = walk(&rec.bids, q);
    assert_eq!(filled, q, "глубины bids записи хватает");
    Prices {
        buy: to_decimal(b.amount_in, 18).unwrap() / q,
        sell: sum / q,
        fees_pct: Decimal::ZERO,
        qty: q,
    }
}

/// Направление DEX → BingX по CAKE.
fn direction() -> Direction {
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE),
        },
        buy_on: VenueId("bsc_dex".into()),
        sell_on: VenueId("bingx".into()),
    }
}

/// Состояние сигнала на ценах: фильтр шума без задержки (держится сразу), аномалия — выше 100 %.
fn state(p: Prices) -> (State, Decimal) {
    let par = Ok(Params {
        hold_ms: 0,
        hold_updates: 1,
        anomaly_pct: d("100"),
    });
    let v = Signals::default().update(&direction(), Ok(p), &par, Utc::now());
    (v.state, v.spread_pct.unwrap())
}

#[test]
fn not_set_is_cap_as_before() {
    for (cap, price, floor) in [
        ("600", "2.189", "2.672"),
        ("40000", "0.000123", "0"),
        ("7", "3", "100"),
    ] {
        let (cap, price, floor) = (d(cap), d(price), d(floor));
        // Побайтно прежняя формула prices.rs: money.max_per_trade_usd / опорная цена.
        assert_eq!(qty(cap, None, price, floor), cap / price);
    }
}

#[tokio::test]
async fn probe_volume_bounds_on_recorded_minimums() {
    let rec = Rec::load("probe-bounds").await;
    let price = rec.asks[0].price;
    // Минимумы записи: minQty 2.672, minNotional 1 USDT, withdrawMin 0.22, depositMin 0.45 → 2.672 монеты.
    let bx_dex = floor_qty([Some(&rec.pair), None], rec.net.withdraw_min, None, price);
    let dex_bx = floor_qty([None, Some(&rec.pair)], None, rec.net.deposit_min, price);
    assert_eq!((bx_dex, dex_bx), (d("2.672"), d("2.672")));
    assert_eq!(floor_qty([None, None], None, None, price), Decimal::ZERO);
    let cap = d("600");
    // Меньше потолка — объём проверки.
    assert_eq!(qty(cap, Some(d("50")), price, bx_dex), d("50") / price);
    // Больше потолка — потолок.
    assert_eq!(qty(cap, Some(d("1000")), price, bx_dex), cap / price);
    // Ниже минимумов площадок — минимум (2.672 монеты, а не 1 / 2.189).
    assert_eq!(qty(cap, Some(d("1")), price, bx_dex), d("2.672"));
    // Минимумы выше потолка — потолок (как раньше: подбор объёма скажет «меньше минимума»).
    assert_eq!(qty(d("5"), Some(d("1")), price, bx_dex), d("5") / price);
}

#[tokio::test]
async fn probe_setting_scope_chain() {
    let p = std::env::temp_dir().join(format!("arb-probe-scope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    let db = Db::open(&p.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = arb::storage::journal::Journal::new(db.clone(), jtx);
    let s = Store::load(db, tx, journal).await.unwrap();
    let pair = Some(VenueDir {
        venue: "bingx",
        kind: DirKind::DexToCex,
    });
    let at = At {
        venue: Some("bsc_dex"),
        coin: None,
    };
    assert_eq!(probe_usd(&s, pair, at).unwrap(), None, "не задана — None");
    let set = |scope: Scope, v: &str| {
        let s = s.clone();
        let v = v.to_string();
        async move {
            s.set("signal.probe_usd", scope, Some(json!(v)), "тест")
                .await
                .unwrap()
        }
    };
    set(Scope::Global, "100").await;
    assert_eq!(probe_usd(&s, pair, at).unwrap(), Some(d("100")));
    set(Scope::Venue("bsc_dex".into()), "70").await;
    assert_eq!(
        probe_usd(&s, pair, at).unwrap(),
        Some(d("70")),
        "площадка A"
    );
    set(Scope::VenueDir("bingx:dex_cex".into()), "50").await;
    assert_eq!(
        probe_usd(&s, pair, at).unwrap(),
        Some(d("50")),
        "пара сильнее"
    );
    // Другая пара — прежняя цепочка.
    let other = Some(VenueDir {
        venue: "bingx",
        kind: DirKind::CexToDex,
    });
    assert_eq!(probe_usd(&s, other, at).unwrap(), Some(d("70")));
    // У «DEX ↔ DEX» объёма проверки нет: сигнал шага 3 цикла не считает (решение 85).
    assert!(
        s.set(
            "signal.probe_usd",
            Scope::VenueDir("dex_dex".into()),
            Some(json!("5")),
            "тест"
        )
        .await
        .is_err()
    );
    let _ = std::fs::remove_dir_all(&p);
}

#[tokio::test]
async fn candidate_appears_on_probe_volume() {
    let rec = Rec::load("probe-signal").await;
    let price = rec.asks[0].price;
    let cap = d("40000");
    let floor = floor_qty([None, Some(&rec.pair)], None, rec.net.deposit_min, price);
    // Объём в шагах пары (как количество котировки).
    let step = |q: Decimal| (q / rec.pair.step_size).floor() * rec.pair.step_size;
    // Не задана — потолок, как раньше: спред минус, «ниже порога».
    let q_cap = step(qty(cap, None, price, floor));
    let (st_cap, sp_cap) = state(dex_to_bingx(&rec, q_cap));
    assert_eq!(st_cap, State::Below, "на потолке {q_cap}: {sp_cap}");
    assert!(sp_cap < Decimal::ZERO);
    // Задана 50 USDT — плюс, кандидат.
    let q_probe = step(qty(cap, Some(d("50")), price, floor));
    let (st_probe, sp_probe) = state(dex_to_bingx(&rec, q_probe));
    assert_eq!(st_probe, State::Candidate, "на {q_probe}: {sp_probe}");
    assert!(sp_probe > sp_cap);
    // На всех точках записи спред с ростом объёма не растёт (средняя цена с объёмом только хуже).
    let mut last: Option<Decimal> = None;
    for q in [
        "2.672", "10", "50", "100", "500", "1000", "5000", "10000", "15000",
    ] {
        let (_, sp) = state(dex_to_bingx(&rec, d(q)));
        if let Some(l) = last {
            assert!(sp <= l, "{q}: {sp} > {l}");
        }
        last = Some(sp);
    }
    println!("потолок {q_cap}: {sp_cap} %; проверка {q_probe}: {sp_probe} %");
}

#[tokio::test]
async fn size_search_range_unchanged() {
    use arb::core::size::Curve;
    let rec = Rec::load("probe-size").await;
    let price = rec.asks[0].price;
    let floor = floor_qty([None, Some(&rec.pair)], None, rec.net.deposit_min, price);
    let step = |q: Decimal| (q / rec.pair.step_size).floor() * rec.pair.step_size;
    let curve = || RecCurve {
        rec: &rec,
        dir: Dir::DexToBingx,
        slip: Decimal::ONE,
    };
    // Бюджет = потолок (лимиты на монету и в пути не меньше). 600 и 10 000 — граница поиска по бюджету; 40 000 — по
    // maxQty пары записи (10689.471).
    for budget in [d("600"), d("10000"), d("40000")] {
        // profit.rs: у DEX на A стакана нет — оценка старта = бюджет / цена покупки из строки сигнала.
        let buy_cap = dex_to_bingx(&rec, step(qty(budget, None, price, floor))).buy;
        let buy_probe = dex_to_bingx(&rec, step(qty(budget, Some(d("50")), price, floor))).buy;
        assert!(buy_probe <= buy_cap, "на меньшем объёме покупка не дороже");
        let mut out = vec![];
        for buy in [buy_cap, buy_probe] {
            let mut p = params(&rec, Dir::DexToBingx, budget);
            p.guess = budget / buy;
            out.push(search(&mut curve(), &p).await.unwrap().choice);
        }
        let (cap, probe) = (&out[0], &out[1]);
        println!("бюджет {budget}\n  потолок: {cap:?}\n  проверка: {probe:?}");
        assert!(cap.max_exact && probe.max_exact);
        assert_eq!(
            probe.max_qty, cap.max_qty,
            "{budget}: тот же наибольший объём"
        );
        assert_eq!(
            (probe.qty, probe.profit),
            (cap.qty, cap.profit),
            "{budget}: тот же выбор"
        );
        assert!(
            probe.evals <= cap.evals,
            "{budget}: расчётов больше: {} > {}",
            probe.evals,
            cap.evals
        );
        if budget < d("40000") {
            // Верхняя граница — бюджет: на шаг больше списание уже больше бюджета.
            let next = curve().at(cap.max_qty + rec.pair.step_size).await.unwrap();
            assert!(next.invested > budget, "{budget}: {}", next.invested);
        }
    }
}
