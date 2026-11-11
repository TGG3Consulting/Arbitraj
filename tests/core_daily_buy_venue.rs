//! Файл: tests/core_daily_buy_venue.rs
//! Что проверяет: решение 173, дополнение t91 (Тигран 11.11.2026: «биржа ↔ биржа — бирже покупки, как в отчёте») на
//! настоящем ядре и SQLite во временной папке (без площадок в реестре шлюза, виды — в inner.kinds; моков нет) и
//! записанных сделках биржа ↔ биржа отчёта 05.11 (tests/data/report_2026-11-05/deals.csv: KM ION −1.3198, BM TWT −0.058,
//! MB CREO +0.2174 и CREO 44.83 USDT):
//! 1) минус сделки биржа ↔ биржа — только бирже покупки: её лимит и пауза; биржа продажи не копит; общий итог прежний;
//!    вход в сделку не менялся — пауза любой из двух бирж направления не даёт начать сделку биржа ↔ биржа;
//! 2) запись итога дня по прежнему правилу (обеим биржам, без venue_rule) — после перезапуска сегодня без двойного
//!    счёта и без потери (по начислениям Deal.day_book и без них — по result_usd), отметка сброса лимита биржи продажи
//!    поправлена, оценка остановленной сделки — у биржи покупки; строка прошлого дня не переписывается;
//! 3) «По биржам» отчёта «Итоги сделок» (report/rows.rs owner) = итог дня бота по площадкам на 400 записанных сделках.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::report, arb::settings, arb::storage, tests/report_common.

mod report_common;

use std::collections::{BTreeMap, BTreeSet};

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::risk::{Mode, can_open};
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use report_common::{d, deal, dir, recorded, start};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Value, json};

/// Площадка по id.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Ядро: виды площадок, обязательные money.* и risk.*, лимит дня площадки `limit` («вся система»), «В работе».
async fn core_at(p: &std::path::Path, limit: &str) -> (Core, Store) {
    let (core, st) = start(p).await;
    *core.inner.kinds.write().unwrap() = vec![
        (v("bingx"), VenueKind::Cex),
        (v("bsc_dex"), VenueKind::Dex),
        (v("gate"), VenueKind::Cex),
        (v("kucoin"), VenueKind::Cex),
        (v("mexc"), VenueKind::Cex),
    ];
    for (n, x) in [
        ("money.min_profit_usd", "0.1"),
        ("money.min_profit_pct", "0.1"),
        ("money.max_per_trade_usd", "50"),
        ("money.max_per_coin_usd", "100"),
        ("money.max_in_transit_usd", "200"),
        ("risk.max_daily_loss_usd", limit),
        ("risk.min_bnb_balance", "0.01"),
    ] {
        st.set(n, Scope::Global, Some(json!(x)), "тест")
            .await
            .unwrap();
    }
    for id in ["bingx", "bsc_dex", "kucoin", "mexc"] {
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
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: v(buy),
        sell_on: v(sell),
    }
}

/// Итог дня площадки (нет строки — 0).
fn res(core: &Core, id: &str) -> Decimal {
    core.control()
        .day
        .venues
        .get(id)
        .map_or(Decimal::ZERO, |x| x.result_usd)
}

/// Сумма итогов дня по площадкам.
fn venues_sum(core: &Core) -> Decimal {
    core.control()
        .day
        .venues
        .values()
        .map(|x| x.result_usd)
        .sum()
}

/// Записанная сделка `k` закрыта ядром (ledger::finish).
async fn close(core: &Core, k: usize) -> Deal {
    let mut x = deal(k, "r");
    arb::core::ledger::finish(core, &mut x, Step::Closed, "запись 05.11").await;
    x
}

#[tokio::test]
async fn cex_cex_loss_goes_to_buy_venue_only() {
    let p = dir("buyvenue");
    let (core, _) = core_at(&p, "1").await;
    // KuCoin → MEXC, ION, −1.3198 (запись 05.11 02:00).
    let x = close(&core, 379).await;
    assert_eq!(
        (
            x.direction.buy_on.0.as_str(),
            x.direction.sell_on.0.as_str()
        ),
        ("kucoin", "mexc")
    );
    assert_eq!(res(&core, "kucoin"), d("-1.3198"));
    assert_eq!(res(&core, "mexc"), Decimal::ZERO, "биржа продажи не копит");
    let r = core.control();
    assert_eq!(r.day.result_usd, d("-1.3198"));
    assert_eq!(
        venues_sum(&core),
        r.day.result_usd,
        "сумма по площадкам = общий итог"
    );
    // Лимит 1 превышен у биржи покупки — пауза только её.
    assert!(r.venue_paused.contains_key(&v("kucoin")));
    assert!(!r.venue_paused.contains_key(&v("mexc")));
    assert!(r.day.venues["kucoin"].paused && !r.day.venues["mexc"].paused);
    assert_eq!(can_open(&core, &dir_of("mexc", "bsc_dex")), Ok(()));
    assert!(
        can_open(&core, &dir_of("kucoin", "bsc_dex"))
            .unwrap_err()
            .contains("на паузе")
    );
    // Вход не менялся: пауза биржи продажи (KuCoin) не даёт начать MEXC → KuCoin.
    assert!(
        can_open(&core, &dir_of("mexc", "kucoin"))
            .unwrap_err()
            .contains("kucoin на паузе")
    );
    // BingX → MEXC, TWT, −0.058: бирже покупки BingX; MEXC — ноль.
    close(&core, 380).await;
    assert_eq!(res(&core, "bingx"), d("-0.0580"));
    assert_eq!(res(&core, "mexc"), Decimal::ZERO);
    // MEXC → BingX, CREO, +0.2174: бирже покупки MEXC; BingX не меняется.
    close(&core, 299).await;
    assert_eq!(res(&core, "mexc"), d("0.2174"));
    assert_eq!(res(&core, "bingx"), d("-0.0580"));
    let r = core.control();
    assert_eq!(r.day.result_usd, d("-1.1604"));
    assert_eq!(venues_sum(&core), r.day.result_usd);
    assert!(!r.venue_paused.contains_key(&v("bingx")) && !r.venue_paused.contains_key(&v("mexc")));
    let _ = std::fs::remove_dir_all(&p);
}

/// Сегодняшняя строка daily_result. Ошибок нет (нет строки — паника теста).
async fn body_today(core: &Core) -> Value {
    let day = chrono::Utc::now().date_naive().to_string();
    let s: String = core
        .inner
        .deps
        .db
        .call(move |c| {
            c.query_row("SELECT body FROM daily_result WHERE day = ?1", [day], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap();
    serde_json::from_str(&s).unwrap()
}

/// Пишет строку daily_result дня `day`. Ошибок нет.
async fn put(core: &Core, day: String, body: &Value) {
    let body = body.to_string();
    core.inner
        .deps
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO daily_result (day, body, updated_at) VALUES (?1, ?2, 't')
                 ON CONFLICT(day) DO UPDATE SET body = excluded.body",
                rusqlite::params![day, body],
            )
            .map(|_| ())
        })
        .await
        .unwrap();
}

/// Decimal из поля JSON-строки книги.
fn num(x: &Value) -> Decimal {
    x.as_str().unwrap().parse().unwrap()
}

/// Сценарий перезапуска после выкладки: сегодня до выкладки (запись по прежнему правилу) → перезапуск.
/// `with_book` — у сделок есть начисления Deal.day_book (иначе — как сделки до решения 257).
async fn old_rule_restart(name: &str, with_book: bool) {
    let p = dir(name);
    let yesterday = (chrono::Utc::now().date_naive() - chrono::Duration::days(1)).to_string();
    let old_y = json!({"final_usd": "-1", "provisional": {}, "venue_final": {"kucoin": "-1", "mexc": "-1"}});
    let (expect, halted_id, base_new, old_body) = {
        let (core, _) = core_at(&p, "100").await;
        put(&core, yesterday.clone(), &old_y).await;
        close(&core, 0).await; // BingX → DEX, CAKE, −0.26
        let a = close(&core, 379).await; // KuCoin → MEXC, −1.3198
        let b = close(&core, 380).await; // BingX → MEXC, −0.058
        // Остановленная MEXC → BingX (CREO 44.83): оценка — у биржи покупки MEXC.
        let mut h = deal(299, "h");
        (h.proceeds, h.qty) = (Decimal::ZERO, d("1"));
        arb::core::ledger::finish(&core, &mut h, Step::Halted, "тест: остановлена").await;
        let est = h.provisional_usd.unwrap();
        assert_eq!(res(&core, "mexc"), est);
        assert_eq!(res(&core, "bingx"), d("-0.3180"));
        core.reset_venue_daily_limit("mexc", "тест").await;
        let mut body = body_today(&core).await;
        assert_eq!(body["venue_rule"], json!(1));
        let base_new = num(&body["venue_resets"]["mexc"]["base_usd"]);
        assert_eq!(base_new, est);
        let expect: BTreeMap<String, Decimal> = core
            .control()
            .day
            .venues
            .iter()
            .map(|(k, x)| (k.clone(), x.result_usd))
            .collect();
        // Запись, как её сделал бы прежний бот: MEXC (продажа KM и BM) получила и их факт, BingX — оценку CREO.
        let o = body.as_object_mut().unwrap();
        o.remove("venue_rule");
        let sells = a.result_usd.unwrap() + b.result_usd.unwrap();
        let vf = o["venue_final"].as_object_mut().unwrap();
        let mx = vf.get("mexc").map_or(Decimal::ZERO, num) + sells;
        vf.insert("mexc".into(), json!(mx.to_string()));
        o["deal_venues"][&h.id] = json!(["mexc", "bingx"]);
        let r = &mut o["venue_resets"]["mexc"];
        r["base_usd"] = json!((base_new + sells).to_string());
        put(&core, chrono::Utc::now().date_naive().to_string(), &body).await;
        if !with_book {
            for mut x in core.deals() {
                x.day_book.clear();
                arb::core::ledger::save(&core, &x).await;
            }
        }
        (expect, h.id, base_new, body)
    };
    // Перезапуск. У бота виды площадок (реестр шлюза) есть до загрузки итога дня (Core::start); у теста реестр пуст и
    // виды задаются после старта — поэтому запись по прежнему правилу кладётся ещё раз и загружается тем же вызовом,
    // что у Core::start (daily::load), уже с видами.
    let (core, _) = core_at(&p, "100").await;
    put(&core, chrono::Utc::now().date_naive().to_string(), &old_body).await;
    arb::core::daily::load(&core).await;
    let r = core.control();
    for (k, x) in &expect {
        assert_eq!(
            res(&core, k),
            *x,
            "{name}: {k} после перезапуска — по новому правилу, без двойного счёта"
        );
    }
    assert_eq!(
        r.day.result_usd,
        expect.values().copied().sum::<Decimal>(),
        "{name}: без потери"
    );
    assert_eq!(venues_sum(&core), r.day.result_usd);
    assert_eq!(r.day.deal_venues[&halted_id], vec!["mexc".to_string()]);
    let reset = r.day.venues["mexc"].limit_reset.clone().unwrap();
    assert_eq!(
        reset.base_usd, base_new,
        "{name}: отметка сброса MEXC без чужого факта"
    );
    assert_eq!(body_today(&core).await["venue_rule"], json!(1));
    // Прошлый день не пересчитан.
    let y: String = core
        .inner
        .deps
        .db
        .call(move |c| {
            c.query_row(
                "SELECT body FROM daily_result WHERE day = ?1",
                [yesterday],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&y).unwrap(), old_y);
    let _ = std::fs::remove_dir_all(&p);
}

#[tokio::test]
async fn old_rule_record_today_after_restart_by_day_book() {
    old_rule_restart("oldrule-book", true).await;
}

#[tokio::test]
async fn old_rule_record_today_after_restart_without_day_book() {
    old_rule_restart("oldrule-nobook", false).await;
}

#[tokio::test]
async fn report_by_venue_matches_bot_day_by_venue() {
    let p = dir("byvenue");
    let (core, _) = core_at(&p, "100000").await;
    let mut all = recorded("r");
    for x in all.iter_mut() {
        arb::core::ledger::finish(&core, x, Step::Closed, "запись 05.11").await;
    }
    let deals = core.deals();
    let w = arb::report::rows_time::Window {
        from: None,
        to: None,
        now: chrono::Utc::now(),
    };
    let dex: BTreeSet<String> = ["bsc_dex".to_string()].into();
    let mut report: BTreeMap<String, f64> = BTreeMap::new();
    for r in arb::report::rows::rows(&deals, &w, &dex) {
        *report.entry(r.owner).or_default() += r.fact.unwrap();
    }
    let day = core.control().day;
    let mut bot: BTreeMap<String, f64> = BTreeMap::new();
    for (k, x) in &day.venues {
        if !x.result_usd.is_zero() {
            bot.insert(arb::notify::human::venue(k), x.result_usd.to_f64().unwrap());
        }
    }
    assert_eq!(
        report.keys().collect::<Vec<_>>(),
        bot.keys().collect::<Vec<_>>()
    );
    for (k, x) in &report {
        assert!((x - bot[k]).abs() < 1e-9, "{k}: отчёт {x}, бот {}", bot[k]);
    }
    let total: Decimal = day.venues.values().map(|x| x.result_usd).sum();
    assert_eq!(total, day.result_usd, "сумма по площадкам = общий итог");
    let _ = std::fs::remove_dir_all(&p);
}
