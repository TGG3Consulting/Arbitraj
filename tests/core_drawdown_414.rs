//! Файл: tests/core_drawdown_414.rs
//! Что проверяет: решение 414 (Тигран 19.11.2026, задача t199) — лимит потерь направления как просадка от ПИКА
//! теоретического баланса направления, на настоящем ядре и SQLite во временной папке (реестр шлюза пуст, виды площадок
//! — в inner.kinds; моков нет) и записанных сделках отчёта 05.11 (tests/data/report_2026-11-05/deals.csv: ALEO
//! MEXC → DEX, WKC DEX → MEXC, CAKE BingX → DEX). Факт счёта площадки — числом в drawdown_snap::apply (как
//! remain_fact.rs apply_fact: площадок в тесте нет), суммы примера — из постановки Тиграна:
//! 1) пример Тиграна: слепок 1000 (чистый), 5 % но не более 30 USDT, сделка −35 → пик 1000, баланс 965, просадка 35 >
//!    30 — пауза только этого направления; «Сбросить лимит» — пауза держится до слепка; слепок 965 → пик 965, пауза
//!    снята; без слепка торговля направлением не включается;
//! 2) слепок по учёту: на MEXC 500 и 500 в идущей сделке MEXC → DEX → 1000; выручка DEX → MEXC, ждущая возврата в
//!    кошелёк (другое направление на том же счёте), — чужое, в слепок не входит;
//! 3) сделка закрылась в −60 до возврата USDT — сразу пауза (итог известен — в балансе); плановые затраты возврата —
//!    из плана, пока возврат не закончен;
//! 4) перезапуск в тот же день — слепок, пик и баланс продолжаются, новый слепок не запрашивается;
//! 5) новый день — баланс переносится (итог нового дня с нуля), пауза по просадке полночью не снимается;
//! 6) предел — меньшее из % × пик и USDT; задано одно — оно.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, tests/report_common.

mod report_common;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::drawdown::{self, Limit, SnapWay};
use arb::core::drawdown_fact::Place;
use arb::core::risk::{Mode, Risk, can_open};
use arb::core::types::{Asset, Direction, VenueId};
use arb::core::usdt_return_state::UsdtReturn;
use arb::gateway::contract::VenueKind;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use report_common::{d, deal, dir, start};
use rust_decimal::Decimal;
use serde_json::json;

const P: &str = "mexc:cex_dex";

/// Площадка по id.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Счёт покупки «MEXC → DEX» — биржа MEXC.
fn mexc() -> Place {
    Place {
        venue: v("mexc"),
        acc: None,
        dex: false,
    }
}

/// Задаёт настройку. Ошибок нет.
async fn set(st: &Store, name: &str, scope: Scope, val: &str) {
    st.set(name, scope, Some(json!(val)), "тест").await.unwrap();
}

/// Виды площадок ядру и хранилищу (как core_daily_pair.rs).
fn kinds(core: &Core, st: &Store) {
    let k = [
        ("bingx", VenueKind::Cex),
        ("bsc_dex", VenueKind::Dex),
        ("mexc", VenueKind::Cex),
    ];
    *core.inner.kinds.write().unwrap() = k.iter().map(|(i, x)| (v(i), *x)).collect();
    st.venue_kinds()
        .set(k.iter().map(|(i, x)| (i.to_string(), *x)));
}

/// Ядро: обязательные money.* и risk.*, пределы просадки «вся система» (pct, usd), «В работе».
async fn core_at(path: &std::path::Path, pct: &str, usd: &str) -> (Core, Store) {
    let (core, st) = start(path).await;
    kinds(&core, &st);
    for (n, x) in [
        ("money.min_profit_usd", "0.1"),
        ("money.min_profit_pct", "0.1"),
        ("money.max_per_trade_usd", "50"),
        ("money.max_per_coin_usd", "100"),
        ("money.max_in_transit_usd", "200"),
        ("risk.min_bnb_balance", "0.01"),
        ("risk.drawdown_pct", pct),
        ("risk.drawdown_max_usd", usd),
    ] {
        set(&st, n, Scope::Global, x).await;
    }
    for id in ["bingx", "bsc_dex", "mexc"] {
        st.set(
            "risk.max_errors_in_row",
            Scope::Venue(id.into()),
            Some(json!(3)),
            "тест",
        )
        .await
        .unwrap();
    }
    core.set_mode(Mode::Running).await;
    (core, st)
}

/// Направление на монете.
fn dir_of(buy: &str, sell: &str) -> Direction {
    Direction {
        asset: Asset {
            ticker: "ALEO".into(),
            bsc_contract: None,
        },
        buy_on: v(buy),
        sell_on: v(sell),
    }
}

/// Записанная сделка ALEO MEXC → DEX (строка 359) с итогом `result` (сумма примера Тиграна) — закрыта ядром.
async fn close_aleo(core: &Core, id: &str, result: &str) -> Deal {
    let mut x = deal(359, id);
    x.proceeds = x.invested + d(result);
    arb::core::ledger::finish(core, &mut x, Step::Closed, "пример Тиграна").await;
    x
}

/// Просадка пары на экране.
fn draw(core: &Core, p: &str) -> drawdown::DrawView {
    core.control().day.pairs[p]
        .draw
        .clone()
        .expect("просадка есть")
}

#[tokio::test]
async fn tigran_example_1000_pause_at_35_reset_965() {
    let (core, _st) = core_at(&dir("dd-example"), "5", "30").await;
    let why = can_open(&core, &dir_of("mexc", "bsc_dex")).unwrap_err();
    assert!(why.contains("ещё нет слепка баланса"), "{why}");
    // Слепок — идущих сделок нет: чистый.
    let s = arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("1000")).await;
    assert_eq!((s.way, s.base_usd), (SnapWay::Clean, d("1000")));
    assert_eq!(can_open(&core, &dir_of("mexc", "bsc_dex")), Ok(()));
    close_aleo(&core, "ex", "-35").await;
    let x = draw(&core, P);
    assert_eq!(
        (x.peak_usd, x.balance_usd, x.drawdown_usd),
        (d("1000"), d("965"), d("35"))
    );
    assert_eq!((x.limit_usd, x.by.as_str()), (d("30"), "usd"));
    assert_eq!(x.pct_usd, Some(d("50")));
    assert!(x.exceeded);
    let r = core.control();
    assert!(r.pair_paused.contains_key(P) && r.draw_paused.contains(P));
    assert!(r.pair_paused[P].contains("просадка от пика 35.00 USDT больше предела 30.00 USDT"));
    let why = can_open(&core, &dir_of("mexc", "bsc_dex")).unwrap_err();
    assert!(why.contains("направление MEXC → DEX на паузе"), "{why}");
    // Старый лимит дня у этой пары не действует.
    assert!(r.day.pairs[P].limit.is_none() && r.day.pairs[P].draw_on);
    // Другое направление со своим слепком торгует.
    arb::core::drawdown_snap::apply(&core, "mexc:cex_cex", &mexc(), "USDT", d("1000")).await;
    assert_eq!(can_open(&core, &dir_of("mexc", "bingx")), Ok(()));
    // «Сбросить лимит»: запрос нового слепка, пауза — до слепка.
    core.reset_venue_daily_limit(P, "тест").await;
    let r = core.control();
    assert!(r.pair_paused.contains_key(P));
    assert!(r.day.pairs[P].draw_want.as_ref().is_some_and(|w| w.human));
    // Слепок 965 → пик 965, пауза снята.
    arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("965")).await;
    let x = draw(&core, P);
    assert_eq!((x.peak_usd, x.drawdown_usd), (d("965"), Decimal::ZERO));
    let r = core.control();
    assert!(!r.pair_paused.contains_key(P) && r.day.pairs[P].draw_want.is_none());
    assert_eq!(can_open(&core, &dir_of("mexc", "bsc_dex")), Ok(()));
}

#[tokio::test]
async fn ledger_snapshot_own_in_flight_minus_foreign_on_same_account() {
    let (core, _st) = core_at(&dir("dd-ledger"), "5", "30").await;
    // Своё: идущая сделка MEXC → DEX, вложено 500 (пример Тиграна), монеты на продаже.
    let mut own = deal(359, "own");
    own.invested = d("500");
    own.step = Step::Selling;
    arb::core::ledger::save(&core, &own).await;
    let s = arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("500")).await;
    assert_eq!((s.way, s.base_usd), (SnapWay::Ledger, d("1000")));
    assert_eq!(s.own, vec![(own.id.clone(), d("500"))]);
    // Чужое: WKC DEX → MEXC закрыта, выручка 44.7143 на MEXC ждёт возврата в кошелёк («идёт»).
    let mut f = deal(108, "foreign");
    f.step = Step::Closed;
    f.result_usd = Some(f.proceeds - f.invested);
    f.usdt_return = Some(UsdtReturn::Pending {
        amount: f.proceeds,
        from: v("mexc"),
        to: v("bsc_dex"),
    });
    arb::core::ledger::save(&core, &f).await;
    // На MEXC по факту теперь 500 + выручка чужой сделки; слепок — снова 1000.
    let s = arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("500") + f.proceeds).await;
    assert_eq!(s.foreign, vec![(f.id.clone(), f.proceeds)]);
    assert_eq!(s.base_usd, d("1000"));
    assert_eq!(draw(&core, P).peak_usd, d("1000"));
}

#[tokio::test]
async fn closed_minus_60_before_usdt_return_pauses() {
    let (core, _st) = core_at(&dir("dd-minus60"), "5", "30").await;
    arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("1000")).await;
    // Сделка ALEO MEXC → DEX на 500 USDT (сумма примера Тиграна; у записанной вложено 39.94 — минус 60 дал бы
    // отрицательную выручку, возвращать было бы нечего), итог −60: выручка 440 ждёт возврата USDT.
    let mut x = deal(359, "m60");
    x.invested = d("500");
    x.proceeds = d("440");
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "пример Тиграна").await;
    let v60 = draw(&core, P);
    assert_eq!((v60.balance_usd, v60.drawdown_usd), (d("940"), d("60")));
    assert!(core.control().pair_paused.contains_key(P));
    // Плановые затраты возврата (план: стадия «возврат USDT» 1.5 USDT) — пока возврат не закончен.
    let mut y = x.clone();
    y.plan = json!({"costs": [{"kind": "UsdtReturn", "stage": "возврат USDT", "amount": "1.5", "asset": "USDT", "usdt": "1.5"}]});
    y.usdt_return = Some(UsdtReturn::Pending {
        amount: y.proceeds,
        from: v("bsc_dex"),
        to: v("mexc"),
    });
    assert_eq!(arb::core::drawdown_ledger::pending_cost(&y), d("1.5"));
    let st = core.control().day.draw[P].clone();
    let lim = Limit {
        pct: Some(d("5")),
        usd: Some(d("30")),
    };
    assert_eq!(drawdown::view(&st, d("1.5"), lim).balance_usd, d("938.5"));
}

#[tokio::test]
async fn restart_same_day_continues() {
    let path = dir("dd-restart");
    {
        let (core, _st) = core_at(&path, "5", "30").await;
        arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("1000")).await;
        close_aleo(&core, "rs", "-10").await;
        assert_eq!(draw(&core, P).balance_usd, d("990"));
    }
    let (core, st) = start(&path).await;
    kinds(&core, &st);
    let r = core.control();
    let x = r.day.pairs[P]
        .draw
        .clone()
        .expect("слепок пережил перезапуск");
    assert_eq!(
        (x.snap.base_usd, x.peak_usd, x.balance_usd),
        (d("1000"), d("1000"), d("990"))
    );
    assert!(
        r.day.pairs[P].draw_want.is_none(),
        "слепок этого дня есть — новый не нужен"
    );
}

#[test]
fn new_day_carries_balance_and_keeps_draw_pause() {
    let mut st = drawdown::fresh(
        arb::core::drawdown::Snap {
            base_usd: d("1000"),
            fact_usd: d("1000"),
            own_usd: Decimal::ZERO,
            foreign_usd: Decimal::ZERO,
            way: SnapWay::Clean,
            at: chrono::Utc::now(),
            day: chrono::Utc::now().date_naive(),
            by: "тест".into(),
            place: "MEXC".into(),
            own: vec![],
            foreign: vec![],
        },
        Decimal::ZERO,
        chrono::Utc::now().date_naive() - chrono::Duration::days(1),
        Decimal::ZERO,
    );
    let y = st.seen_day;
    drawdown::advance(&mut st, d("-35"), y, Decimal::ZERO);
    let today = y.succ_opt().unwrap();
    // Новый день: итог дня с нуля — баланс тот же; новый минус копится дальше.
    drawdown::advance(&mut st, Decimal::ZERO, today, Decimal::ZERO);
    assert_eq!((st.balance_usd, st.peak_usd), (d("965"), d("1000")));
    drawdown::advance(&mut st, d("-10"), today, Decimal::ZERO);
    assert_eq!(st.balance_usd, d("955"));
    // Полночь: пауза по просадке остаётся (снимет новый слепок), по старому лимиту — снимается.
    let mut r = Risk::default();
    r.pair_paused.insert(P.into(), "просадка".into());
    r.draw_paused.insert(P.into());
    r.pair_paused
        .insert("bingx:cex_dex".into(), "убыток".into());
    let lift = arb::core::day_roll::new_day(&mut r);
    assert_eq!(lift.pairs, vec!["bingx:cex_dex".to_string()]);
    assert!(r.pair_paused.contains_key(P));
}

#[test]
fn limit_is_min_of_pct_and_usd() {
    let mut st = drawdown::fresh(
        arb::core::drawdown::Snap {
            base_usd: d("1000"),
            fact_usd: d("1000"),
            own_usd: Decimal::ZERO,
            foreign_usd: Decimal::ZERO,
            way: SnapWay::Clean,
            at: chrono::Utc::now(),
            day: chrono::Utc::now().date_naive(),
            by: "тест".into(),
            place: "MEXC".into(),
            own: vec![],
            foreign: vec![],
        },
        Decimal::ZERO,
        chrono::Utc::now().date_naive(),
        Decimal::ZERO,
    );
    let day = st.seen_day;
    // Сначала +100 — пик 1100, затем −40 от пика.
    drawdown::advance(&mut st, d("100"), day, Decimal::ZERO);
    drawdown::advance(&mut st, d("60"), day, Decimal::ZERO);
    let l = |pct: Option<&str>, usd: Option<&str>| Limit {
        pct: pct.map(d),
        usd: usd.map(d),
    };
    let a = drawdown::view(&st, Decimal::ZERO, l(Some("5"), Some("30")));
    assert_eq!(
        (a.peak_usd, a.drawdown_usd, a.limit_usd),
        (d("1100"), d("40"), d("30"))
    );
    assert!(a.exceeded);
    let b = drawdown::view(&st, Decimal::ZERO, l(Some("5"), None));
    assert_eq!(
        (b.limit_usd, b.by.as_str(), b.exceeded),
        (d("55"), "pct", false)
    );
    let c = drawdown::view(&st, Decimal::ZERO, l(None, Some("40")));
    assert!(!c.exceeded, "просадка 40 не больше предела 40");
}
