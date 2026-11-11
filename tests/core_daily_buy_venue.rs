//! Файл: tests/core_daily_buy_venue.rs
//! Что проверяет: решение 173, дополнение t91 (Тигран 11.11.2026: «биржа ↔ биржа — бирже покупки, как в отчёте») и
//! решение 258 (Тигран 12.11.2026: «Каждое направление имеет свой лимит. Все.» — итог и лимит по паре «площадка × тип
//! направления») на настоящем ядре и SQLite во временной папке (без площадок в реестре шлюза, виды — в inner.kinds;
//! моков нет) и записанных сделках биржа ↔ биржа отчёта 05.11 (tests/data/report_2026-11-05/deals.csv: KM ION −1.3198,
//! BM TWT −0.058, MB CREO +0.2174 и CREO 44.83 USDT):
//! 1) минус сделки биржа ↔ биржа — только паре «биржа покупки → биржа»: её лимит и пауза; биржа продажи не копит; общий
//!    итог прежний; решение 258 — пауза только этого направления: другие направления биржи покупки и сделки, где она
//!    продаёт, торгуют;
//! 2) запись итога дня до решения 258 (по площадкам: t91 — бирже покупки; решение 173 — обеим биржам; с начислениями
//!    Deal.day_book и без них) — после перезапуска разносится по парам сделок без двойного счёта и без потери, сброс
//!    лимита площадки переходит к её парам (итог пары в момент сброса), оценка остановленной сделки — у её пары;
//!    строка прошлого дня не переписывается;
//! 3) «По биржам» отчёта «Итоги сделок» (report/rows.rs owner — направление итога) = итог дня бота по парам на 400
//!    записанных сделках.
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

/// Итог дня пары «площадка × тип направления» (нет строки — 0).
fn res(core: &Core, id: &str) -> Decimal {
    core.control()
        .day
        .pairs
        .get(id)
        .map_or(Decimal::ZERO, |x| x.result_usd)
}

/// Итог дня всех пар биржи (сумма её трёх направлений).
fn venue_res(core: &Core, v: &str) -> Decimal {
    ["cex_dex", "dex_cex", "cex_cex"]
        .iter()
        .map(|k| res(core, &format!("{v}:{k}")))
        .sum()
}

/// Сумма итогов дня по парам.
fn venues_sum(core: &Core) -> Decimal {
    core.control()
        .day
        .pairs
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
    assert_eq!(res(&core, "kucoin:cex_cex"), d("-1.3198"));
    assert_eq!(
        venue_res(&core, "mexc"),
        Decimal::ZERO,
        "биржа продажи не копит"
    );
    let r = core.control();
    assert_eq!(r.day.result_usd, d("-1.3198"));
    assert_eq!(
        venues_sum(&core),
        r.day.result_usd,
        "сумма по площадкам = общий итог"
    );
    // Лимит 1 превышен у пары «KuCoin → биржа» — пауза только этого направления (решение 258; было — вся KuCoin).
    assert!(r.pair_paused.contains_key("kucoin:cex_cex"));
    assert_eq!(r.pair_paused.len(), 1);
    assert!(r.venue_paused.is_empty());
    assert!(r.day.pairs["kucoin:cex_cex"].paused && !r.day.pairs["mexc:cex_cex"].paused);
    assert_eq!(can_open(&core, &dir_of("mexc", "bsc_dex")), Ok(()));
    // Другие направления KuCoin торгуют: KuCoin → DEX, DEX → KuCoin (было — пауза всей KuCoin).
    assert_eq!(can_open(&core, &dir_of("kucoin", "bsc_dex")), Ok(()));
    assert_eq!(can_open(&core, &dir_of("bsc_dex", "kucoin")), Ok(()));
    // Покупка на KuCoin и продажа на другой бирже — не начинается (любая биржа продажи).
    for sell in ["mexc", "bingx"] {
        assert!(
            can_open(&core, &dir_of("kucoin", sell))
                .unwrap_err()
                .contains("направление KuCoin → биржа на паузе")
        );
    }
    // MEXC → KuCoin — это «MEXC → биржа»: торгует (было: пауза KuCoin как биржи продажи не давала начать).
    assert_eq!(can_open(&core, &dir_of("mexc", "kucoin")), Ok(()));
    // BingX → MEXC, TWT, −0.058: паре «BingX → биржа»; MEXC — ноль.
    close(&core, 380).await;
    assert_eq!(res(&core, "bingx:cex_cex"), d("-0.0580"));
    assert_eq!(venue_res(&core, "mexc"), Decimal::ZERO);
    // MEXC → BingX, CREO, +0.2174: паре «MEXC → биржа»; BingX не меняется.
    close(&core, 299).await;
    assert_eq!(res(&core, "mexc:cex_cex"), d("0.2174"));
    assert_eq!(venue_res(&core, "bingx"), d("-0.0580"));
    let r = core.control();
    assert_eq!(r.day.result_usd, d("-1.1604"));
    assert_eq!(venues_sum(&core), r.day.result_usd);
    assert_eq!(r.pair_paused.len(), 1);
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

/// Сценарий перезапуска после выкладки: сегодня до выкладки (запись по площадкам, до решения 258) → перезапуск.
/// `with_book` — у сделок есть начисления Deal.day_book (иначе — как сделки до решения 257); `rule0` — запись по
/// решению 173 (биржа ↔ биржа — обеим биржам, без venue_rule), иначе — по t91 (venue_rule = 1).
async fn old_record_restart(name: &str, with_book: bool, rule0: bool) {
    let p = dir(name);
    let yesterday = (chrono::Utc::now().date_naive() - chrono::Duration::days(1)).to_string();
    let old_y = json!({"final_usd": "-1", "provisional": {}, "venue_final": {"kucoin": "-1", "mexc": "-1"}});
    let (expect, halted_id, est, old_body) = {
        let (core, _) = core_at(&p, "100").await;
        put(&core, yesterday.clone(), &old_y).await;
        close(&core, 0).await; // BingX → DEX, CAKE, −0.26
        let a = close(&core, 379).await; // KuCoin → MEXC, −1.3198
        let b = close(&core, 380).await; // BingX → MEXC, −0.058
        // Остановленная MEXC → BingX (CREO 44.83): оценка — у пары «MEXC → биржа».
        let mut h = deal(299, "h");
        (h.proceeds, h.qty) = (Decimal::ZERO, d("1"));
        arb::core::ledger::finish(&core, &mut h, Step::Halted, "тест: остановлена").await;
        let est = h.provisional_usd.unwrap();
        assert_eq!(res(&core, "mexc:cex_cex"), est);
        assert_eq!(res(&core, "bingx:cex_dex"), d("-0.26"));
        assert_eq!(res(&core, "bingx:cex_cex"), d("-0.0580"));
        core.reset_venue_daily_limit("mexc", "тест").await;
        let body = body_today(&core).await;
        assert!(body.get("pair_final").is_some(), "новая запись — по парам");
        assert_eq!(num(&body["pair_resets"]["mexc:cex_cex"]["base_usd"]), est);
        let expect: BTreeMap<String, Decimal> = core
            .control()
            .day
            .pairs
            .iter()
            .map(|(k, x)| (k.clone(), x.result_usd))
            .collect();
        // Запись, как её сделал бы бот до решения 258: по площадкам.
        let reset = body["pair_resets"]["mexc:cex_cex"].clone();
        let sells = a.result_usd.unwrap() + b.result_usd.unwrap();
        let mut vf = json!({"bingx": "-0.3180", "kucoin": "-1.3198"});
        if rule0 {
            // Решение 173: биржа ↔ биржа — и бирже продажи (MEXC получила факт KM и BM), оценка CREO — обеим.
            vf["mexc"] = json!(sells.to_string());
        }
        let mut old = json!({
            "final_usd": body["final_usd"].clone(),
            "provisional": body["provisional"].clone(),
            "unpriced": body["unpriced"].clone(),
            "venue_final": vf,
            "deal_venues": {h.id.clone(): if rule0 { json!(["mexc", "bingx"]) } else { json!(["mexc"]) }},
            "venue_resets": {"mexc": reset},
        });
        if !rule0 {
            old["venue_rule"] = json!(1);
        }
        put(&core, chrono::Utc::now().date_naive().to_string(), &old).await;
        if !with_book {
            for mut x in core.deals() {
                x.day_book.clear();
                arb::core::ledger::save(&core, &x).await;
            }
        }
        (expect, h.id, est, old)
    };
    // Перезапуск. У бота виды площадок (реестр шлюза) есть до загрузки итога дня (Core::start); у теста реестр пуст и
    // виды задаются после старта — поэтому запись по площадкам кладётся ещё раз и загружается тем же вызовом, что у
    // Core::start (daily::load), уже с видами.
    let (core, _) = core_at(&p, "100").await;
    put(
        &core,
        chrono::Utc::now().date_naive().to_string(),
        &old_body,
    )
    .await;
    arb::core::daily::load(&core).await;
    let r = core.control();
    for (k, x) in &expect {
        assert_eq!(
            res(&core, k),
            *x,
            "{name}: {k} после перезапуска — по парам, без двойного счёта"
        );
    }
    assert_eq!(
        r.day.result_usd,
        expect.values().copied().sum::<Decimal>(),
        "{name}: без потери"
    );
    assert_eq!(venues_sum(&core), r.day.result_usd);
    assert_eq!(r.day.deal_pairs[&halted_id], "mexc:cex_cex");
    // Сброс площадки MEXC — у каждой её пары; итог пары в момент сброса — её начисления до отметки.
    for (k, base) in [
        ("mexc:cex_cex", est),
        ("mexc:cex_dex", Decimal::ZERO),
        ("mexc:dex_cex", Decimal::ZERO),
    ] {
        let reset = r.day.pairs[k].limit_reset.clone().unwrap();
        assert_eq!(reset.base_usd, base, "{name}: отметка сброса {k}");
    }
    assert!(r.day.pairs["bingx:cex_cex"].limit_reset.is_none());
    // Паники и ложной паузы нет (лимит 100).
    assert!(r.pair_paused.is_empty());
    assert!(body_today(&core).await.get("pair_final").is_some());
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
    old_record_restart("oldrule-book", true, false).await;
}

#[tokio::test]
async fn old_rule_record_today_after_restart_without_day_book() {
    old_record_restart("oldrule-nobook", false, true).await;
}

#[tokio::test]
async fn rule0_record_with_day_book_no_double_count() {
    old_record_restart("rule0-book", true, true).await;
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
    for x in day.pairs.values() {
        if !x.result_usd.is_zero() {
            bot.insert(x.label.clone(), x.result_usd.to_f64().unwrap());
        }
    }
    assert_eq!(
        report.keys().collect::<Vec<_>>(),
        bot.keys().collect::<Vec<_>>()
    );
    for (k, x) in &report {
        assert!((x - bot[k]).abs() < 1e-9, "{k}: отчёт {x}, бот {}", bot[k]);
    }
    let total: Decimal = day.pairs.values().map(|x| x.result_usd).sum();
    assert_eq!(total, day.result_usd, "сумма по парам = общий итог");
    let _ = std::fs::remove_dir_all(&p);
}
