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
//! 3) сделка закрылась в −60 до возврата USDT — сразу пауза (итог известен — в балансе);
//! 4) перезапуск в тот же день — слепок, пик и баланс продолжаются, новый слепок не запрашивается;
//! 5) новый день — баланс переносится (итог нового дня с нуля), пауза по просадке полночью не снимается;
//! 6) предел — меньшее из % × пик и USDT; задано одно — оно;
//!
//! Ревью t199 и повторное ревью:
//! 7) С1 — плановые затраты возврата закрываемой сделки (usdt_return.mode = per_trade, план 1.5) — уже на закрытии:
//!    −29 при пределе 30 → просадка 30.5 — пауза; −60 → баланс 938.5;
//! 8) С3 — пауза старого лимита дня у направления, перешедшего на просадку, снимается «Сбросить лимит» и слепком;
//! 9) утренний слепок снимает паузу по просадке;
//! 10) пауза по просадке и «Снять паузу» переживают перезапуск;
//! 11) С1′ — затраты «как при подготовке возврата» только в самом закрытии и не у сделки в чужом общем возврате группы;
//!     закрываемая сделка (Inner.draw_closing) видна любому пересчёту до сохранения;
//! 12) пауза старого лимита при превышенной просадке получает отметку «по просадке»;
//! 13) снизили предел ниже просадки — пауза сразу;
//! 14) финальное ревью: после «Снять паузу» смена настройки другого направления паузу не возвращает, снижение предела
//!     своего — возвращает сразу.
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
    let (core, st) = base(path).await;
    set(&st, "risk.drawdown_pct", Scope::Global, pct).await;
    set(&st, "risk.drawdown_max_usd", Scope::Global, usd).await;
    (core, st)
}

/// Ядро без пределов просадки: обязательные money.* и risk.*, «В работе» (лимит дня — задаёт тест).
async fn base(path: &std::path::Path) -> (Core, Store) {
    let (core, st) = start(path).await;
    kinds(&core, &st);
    for (n, x) in [
        ("money.min_profit_usd", "0.1"),
        ("money.min_profit_pct", "0.1"),
        ("money.max_per_trade_usd", "50"),
        ("money.max_per_coin_usd", "100"),
        ("money.max_in_transit_usd", "200"),
        ("risk.min_bnb_balance", "0.01"),
    ] {
        set(&st, n, Scope::Global, x).await;
    }
    // Решение 416: тикер газа площадки DEX — настройка (dex.gas_ticker).
    st.set(
        "dex.gas_ticker",
        Scope::Venue("bsc_dex".into()),
        Some(json!("BNB")),
        "тест",
    )
    .await
    .unwrap();
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
            chain: None,
            ticker: "ALEO".into(),
            contract: None,
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

/// Сделка ALEO MEXC → DEX (записанная строка 359) на 500 USDT (сумма примера Тиграна; у записанной вложено 39.94 —
/// минус 60 дал бы отрицательную выручку), продана на DEX с итогом `result`; в плане — затраты стадии «возврат USDT»
/// 1.5 USDT. Возврат включён (usdt_return.mode = per_trade): закрытие ядром ставит часть возврата (кошелька в
/// маппинге нет — «не вернулись», возврат не закончен).
async fn close_with_return(core: &Core, id: &str, result: &str) -> Deal {
    let mut x = deal(359, id);
    x.invested = d("500");
    x.proceeds = d("500") + d(result);
    x.at = v("bsc_dex");
    x.plan = json!({"costs": [{"kind": "UsdtReturn", "stage": "возврат USDT", "amount": "1.5", "asset": "USDT", "usdt": "1.5"}]});
    arb::core::ledger::finish(core, &mut x, Step::Closed, "пример Тиграна").await;
    x
}

#[tokio::test]
async fn closed_minus_60_before_usdt_return_pauses() {
    let (core, st) = core_at(&dir("dd-minus60"), "5", "30").await;
    set(&st, "usdt_return.mode", Scope::Global, "per_trade").await;
    arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("1000")).await;
    let x = close_with_return(&core, "m60", "-60").await;
    // Итог известен — сразу в балансе; плановые затраты возврата 1.5 — уже на закрытии (ревью С1).
    let v60 = draw(&core, P);
    assert_eq!(
        (v60.balance_usd, v60.drawdown_usd, v60.pending_cost_usd),
        (d("938.5"), d("61.5"), d("1.5"))
    );
    assert!(core.control().pair_paused.contains_key(P));
    // Возврат не закончен — часть в учёте сделки, затраты по её доле те же.
    let saved = core.inner.deals.lock().unwrap()[&x.id].clone();
    assert!(!arb::core::usdt_return_parts::all(&saved).is_empty());
    assert_eq!(arb::core::drawdown_pending::pending_cost(&saved), d("1.5"));
}

#[tokio::test]
async fn planned_return_cost_at_close_trips_limit() {
    // Ревью С1: −29 при пределе 30 — без плановых затрат возврата паузы нет, с ними (1.5) просадка 30.5 > 30.
    let (core, st) = core_at(&dir("dd-minus29"), "5", "30").await;
    set(&st, "usdt_return.mode", Scope::Global, "per_trade").await;
    arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("1000")).await;
    close_with_return(&core, "m29", "-29").await;
    let x = draw(&core, P);
    assert_eq!((x.peak_usd, x.drawdown_usd), (d("1000"), d("30.5")));
    assert!(core.control().draw_paused.contains(P));
}

#[tokio::test]
async fn legacy_pause_lifted_by_reset_and_by_snapshot() {
    // Ревью С3: пауза старого лимита дня (risk.max_daily_loss_usd 5, ALEO −35), затем заданы пределы просадки.
    for (name, by_reset) in [("dd-legacy-reset", true), ("dd-legacy-snap", false)] {
        let (core, st) = base(&dir(name)).await;
        set(&st, "risk.max_daily_loss_usd", Scope::Global, "5").await;
        close_aleo(&core, name, "-35").await;
        assert!(core.control().pair_paused.contains_key(P));
        set(&st, "risk.drawdown_pct", Scope::Global, "5").await;
        set(&st, "risk.drawdown_max_usd", Scope::Global, "30").await;
        let r = core.control();
        assert!(r.day.pairs[P].draw_on && r.day.pairs[P].paused && !r.day.pairs[P].draw_paused);
        if by_reset {
            core.reset_venue_daily_limit(P, "тест").await;
            assert!(
                !core.control().pair_paused.contains_key(P),
                "сброс снял старую паузу"
            );
        } else {
            arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("965")).await;
            assert!(
                !core.control().pair_paused.contains_key(P),
                "слепок снял старую паузу"
            );
        }
    }
}

#[tokio::test]
async fn morning_snapshot_lifts_draw_pause() {
    let (core, _st) = core_at(&dir("dd-morning"), "5", "30").await;
    arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("1000")).await;
    close_aleo(&core, "mo", "-35").await;
    // Полночь паузу по просадке не снимает; утренний слепок (965) — снимает, пик = 965.
    arb::core::day_roll::new_day(&mut core.inner.risk.lock().unwrap());
    assert!(core.control().pair_paused.contains_key(P));
    arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("965")).await;
    let r = core.control();
    assert!(!r.pair_paused.contains_key(P) && !r.draw_paused.contains(P));
    assert_eq!(draw(&core, P).peak_usd, d("965"));
}

#[tokio::test]
async fn restart_keeps_draw_pause_and_resume() {
    // Ревью, мелкое 6: пауза по просадке переживает перезапуск; «Снять паузу» тоже (повторной паузы нет).
    let path = dir("dd-restart-pause");
    {
        let (core, _st) = core_at(&path, "5", "30").await;
        arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("1000")).await;
        close_aleo(&core, "rp", "-35").await;
        assert!(core.control().draw_paused.contains(P));
    }
    let (core, st) = start(&path).await;
    kinds(&core, &st);
    assert!(
        core.control().pair_paused.contains_key(P),
        "пауза по просадке пережила перезапуск"
    );
    assert!(arb::core::risk::resume_pair(&core, P));
    // Запись снятой паузы в учёт — тем же пересчётом, что делает задача drawdown_limit::persist (здесь — дождаться его).
    arb::core::drawdown_limit::save(&core).await;
    drop(core);
    let (core, st) = start(&path).await;
    kinds(&core, &st);
    assert!(
        !core.control().pair_paused.contains_key(P),
        "снятая пауза не вернулась"
    );
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

#[tokio::test]
async fn fresh_cost_only_at_close_and_not_for_shared_group_return() {
    // Повторное ревью, С1′: поправка закрытой сделки (settle_closed) и сделка в общем возврате ведущей — без плана.
    let (core, st) = core_at(&dir("dd-fresh"), "5", "30").await;
    set(&st, "usdt_return.mode", Scope::Global, "per_trade").await;
    let mut x = deal(359, "fr");
    x.invested = d("500");
    x.proceeds = d("471");
    x.at = v("bsc_dex");
    x.step = Step::Closed;
    x.plan = json!({"costs": [{"kind": "UsdtReturn", "stage": "возврат USDT", "amount": "1.5", "asset": "USDT", "usdt": "1.5"}]});
    use arb::core::drawdown_pending::{closing, cur, pending};
    assert_eq!(cur(&core, &x, true).cost, d("1.5"));
    assert_eq!(cur(&core, &x, false).cost, Decimal::ZERO);
    let mut g = x.clone();
    g.group = Some(Box::new(arb::core::deal_group::DealGroup {
        id: "lead".into(),
        shared_in: Some("lead".into()),
        ..Default::default()
    }));
    assert_eq!(cur(&core, &g, true).cost, Decimal::ZERO);
    // Остаток С1: от итога дня до сохранения закрываемая сделка видна пересчётам других сделок.
    closing(&core, &x.id, Some(cur(&core, &x, true)));
    assert_eq!(pending(&core, None).get(P).copied(), Some(d("1.5")));
    closing(&core, &x.id, None);
    assert_eq!(pending(&core, None).get(P), None);
}

#[tokio::test]
async fn legacy_pause_with_exceeded_drawdown_gets_draw_mark() {
    // Повторное ревью: слепок 1000 → направление вернули на старый лимит (5) → −35: пауза старого лимита; снова заданы
    // пределы просадки (30) — просадка 35 > 30: отметка «по просадке» поверх, «Сбросить лимит» её не снимает сразу.
    let (core, st) = base(&dir("dd-legacy-over")).await;
    set(&st, "risk.max_daily_loss_usd", Scope::Global, "5").await;
    set(&st, "risk.drawdown_max_usd", Scope::Global, "30").await;
    arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("1000")).await;
    st.set("risk.drawdown_max_usd", Scope::Global, None, "тест")
        .await
        .unwrap();
    close_aleo(&core, "lo", "-35").await;
    let r = core.control();
    assert!(r.pair_paused.contains_key(P) && !r.draw_paused.contains(P));
    set(&st, "risk.drawdown_max_usd", Scope::Global, "30").await;
    arb::core::drawdown_limit::save(&core).await;
    let r = core.control();
    assert!(r.draw_paused.contains(P), "пауза по просадке поверх старой");
    assert!(r.pair_paused[P].contains("просадка от пика 35.00 USDT больше предела 30.00 USDT"));
    core.reset_venue_daily_limit(P, "тест").await;
    assert!(
        core.control().pair_paused.contains_key(P),
        "сброс ждёт слепка"
    );
}

#[tokio::test]
async fn lowered_limit_pauses_at_once() {
    // Повторное ревью: просадка 20 при пределе 30 — торгует; предел снизили до 10 — пауза сразу (наблюдатель настроек
    // забывает замеченную просадку и пересчитывает).
    let (core, st) = core_at(&dir("dd-lower"), "5", "30").await;
    arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("1000")).await;
    close_aleo(&core, "lw", "-20").await;
    assert!(!core.control().pair_paused.contains_key(P));
    set(&st, "risk.drawdown_max_usd", Scope::Global, "10").await;
    // Ждём по признаку: наблюдатель настроек — своя задача.
    for _ in 0..200 {
        if core.control().draw_paused.contains(P) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(core.control().draw_paused.contains(P));
}

#[tokio::test]
async fn resume_survives_other_direction_setting_but_not_own_lower_limit() {
    // Финальное ревью: пауза A → «Снять паузу» → сменили risk.drawdown_pct у B — A без паузы (просадка не росла);
    // снизили предел самой A ниже просадки — пауза сразу.
    let (core, st) = core_at(&dir("dd-seen-limit"), "5", "30").await;
    arb::core::drawdown_snap::apply(&core, P, &mexc(), "USDT", d("1000")).await;
    close_aleo(&core, "sl", "-35").await;
    assert!(core.control().draw_paused.contains(P));
    assert!(arb::core::risk::resume_pair(&core, P));
    arb::core::drawdown_limit::save(&core).await;
    assert!(!core.control().pair_paused.contains_key(P));
    set(
        &st,
        "risk.drawdown_pct",
        Scope::VenueDir("bingx:cex_dex".into()),
        "4",
    )
    .await;
    arb::core::drawdown_limit::save(&core).await;
    assert!(
        !core.control().pair_paused.contains_key(P),
        "настройка другого направления паузу не вернула"
    );
    set(
        &st,
        "risk.drawdown_max_usd",
        Scope::VenueDir(P.into()),
        "20",
    )
    .await;
    arb::core::drawdown_limit::save(&core).await;
    let r = core.control();
    assert!(
        r.draw_paused.contains(P),
        "предел A снизили ниже просадки — пауза сразу"
    );
    assert!(r.pair_paused[P].contains("больше предела 20.00 USDT"));
}
