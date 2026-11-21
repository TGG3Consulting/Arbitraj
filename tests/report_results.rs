//! Файл: tests/report_results.rs
//! Что проверяет: решение 257 — отчёт «Итоги сделок» на настоящей SQLite (ядро без площадок в реестре):
//! 1) итог дня = итог дня бота: 400 записанных сделок (tests/data/report_2026-11-05/deals.csv) закрываются тем же
//!    вызовом, что у бота (ledger::finish), плюс остановленная, «Беру на себя», вложение 0 с газом 0.0021 (живой газ
//!    кошелька DEX, тест 236), возврат USDT «вернулись» после закрытия (живой случай MEXC: ушло 27.6, зачислено 27.59) и
//!    сделка до 257 с оценкой вчера; начисления «Учёта дня» за сегодня = DayView и записи daily_result, кроме строки
//!    «восстановлено» — она ровно равна разнице;
//! 2) несколько дней и старый формат учёта (проверки — tests/report_old_format.rs) — в книге примера;
//! 3) настоящий план (расчёт ядра на записях CAKE, tests/report_plan): запас и возврат USDT плана в колонках T / U;
//! 4) слепок по-настоящему (report::snap::gather): кошелёк DEX при неподключённой DEX — «нет данных», сделка на шаге
//!    перевода — в «В пути», сделка на площадке вне реестра — свой блок «нет данных»;
//! 5) порог «Сделок дольше …» — настройка report.long_deal_s.
//!
//! Книга — в REPORT_OUT (если задан) для пересчёта LibreOffice и сверки в Python.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::report, tests/report_common, tests/report_plan, tests/size_common. Моков нет.

mod report_common;
mod report_plan;
mod size_common;

use std::collections::BTreeSet;

use arb::core::deal::Step;
use arb::core::types::{CostKind, VenueId};
use arb::core::usdt_return_state::UsdtReturn;
use arb::report::rows_time::{Window, calendar_from};
use arb::settings::value::Scope;
use chrono::Utc;
use report_common::{d, deal, dir, old_format, old_restart, recorded, start, yesterday};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::json;

#[tokio::test]
async fn day_matches_bot_and_book_builds() {
    let p = dir("day");
    yesterday(&p).await;
    let (core, st) = start(&p).await;
    for (k, v) in [
        ("report.snapshot_timeout_ms", 5000),
        ("report.long_deal_s", 300),
    ] {
        st.set(k, Scope::Global, Some(json!(v)), "тест")
            .await
            .unwrap();
    }
    st.accounts()
        .add("w1", "BingX", "s1", "q1", "тест")
        .await
        .unwrap();
    let mut all = recorded("r");
    for x in all.iter_mut() {
        arb::core::ledger::finish(&core, x, Step::Closed, "запись 05.11").await;
    }
    let mut h = deal(0, "h");
    h.proceeds = Decimal::ZERO;
    arb::core::ledger::finish(&core, &mut h, Step::Halted, "тест: остановлена").await;
    let mut t = deal(1, "t");
    (t.proceeds, t.qty) = (Decimal::ZERO, d("1"));
    arb::core::ledger::finish(&core, &mut t, Step::TakenOver, "тест: Беру на себя").await;
    let mut z = deal(2, "z");
    (z.invested, z.proceeds) = (Decimal::ZERO, Decimal::ZERO);
    arb::core::deal_book::spend(&mut z, CostKind::Gas, d("0.0021"));
    arb::core::ledger::finish(&core, &mut z, Step::Closed, "тест: покупка не прошла").await;
    let mut r = deal(3, "u");
    r.direction.buy_on = VenueId("bsc_dex".into());
    r.direction.sell_on = VenueId("mexc".into());
    let amount = d("27.655454087068799999999999694");
    let (mx, dx) = (VenueId("mexc".into()), VenueId("bsc_dex".into()));
    r.usdt_return = Some(UsdtReturn::Pending {
        amount,
        from: mx.clone(),
        to: dx.clone(),
    });
    arb::core::ledger::finish(&core, &mut r, Step::Closed, "тест: продано").await;
    let done = UsdtReturn::Done {
        amount,
        credited: d("27.59"),
        from: mx,
        to: dx,
        sent: Some(d("27.6")),
        fee_usd: None,
    };
    arb::core::usdt_return_state::set(&core, &r.id, done, "вернулись").await;
    let mut o = deal(4, "x");
    o.id = "old1".into();
    arb::core::ledger::finish(&core, &mut o, Step::Closed, "тест: разбор").await;
    // Настоящий план: 10 CAKE bingx → DEX, исполнено как в плане.
    let calc = report_plan::plan().await;
    let mut pl = deal(5, "p");
    pl.direction = report_plan::dir();
    (pl.plan, pl.plan_profit_usd) = (serde_json::to_value(&calc).unwrap(), calc.profit_usd);
    (pl.invested, pl.proceeds) = (calc.invested, calc.proceeds);
    arb::core::ledger::finish(&core, &mut pl, Step::Closed, "тест: план").await;
    // Открытые: на шаге перевода (монеты в пути) и на площадке вне реестра (MEXC).
    let mut tr = deal(6, "tr");
    (tr.step, tr.qty) = (Step::Transfer, d("1"));
    arb::core::ledger::save(&core, &tr).await;
    // Возврат USDT «идёт» (не закончен): в слепке — «В пути» под вопросом, сумма части (живой случай MEXC).
    let mut q = deal(9, "q");
    q.usdt_return = Some(UsdtReturn::Pending {
        amount,
        from: VenueId("mexc".into()),
        to: VenueId("bsc_dex".into()),
    });
    arb::core::ledger::finish(&core, &mut q, Step::Closed, "тест: возврат идёт").await;
    let mut ms = deal(7, "ms");
    (ms.step, ms.at, ms.qty) = (Step::Selling, VenueId("mexc".into()), d("1"));
    arb::core::ledger::save(&core, &ms).await;

    let deals = core.deals();
    let now = Utc::now();
    let w = Window {
        from: Some(calendar_from(1, now)),
        to: None,
        now,
    };
    let dex: BTreeSet<String> = ["bsc_dex".to_string()].into();
    let day = core.control().day;
    let prov_days = day
        .provisional
        .iter()
        .map(|(k, (_, dd))| (k.clone(), *dd))
        .collect();
    let ledger = arb::report::day_rows::day_rows(&deals, &w, &dex, &prov_days);
    let today = now.date_naive();
    let sum = |fact: bool, booked: Option<bool>| -> f64 {
        let ok = |x: &&arb::report::day_rows::DayRow| {
            x.date == today && x.fact == fact && booked.is_none_or(|b| x.booked == b)
        };
        ledger.iter().filter(ok).map(|x| x.amount).sum()
    };
    assert!(
        (sum(true, Some(false)) + 0.26).abs() < 1e-9,
        "восстановлено = вчерашняя оценка −0.26"
    );
    let f = |x: Decimal| x.to_f64().unwrap();
    assert!(
        (sum(true, Some(true)) - f(day.final_usd)).abs() < 1e-9,
        "факт дня"
    );
    assert!(
        (sum(false, None) - f(day.provisional_usd)).abs() < 1e-9,
        "оценка дня"
    );
    let (bot, bad) = core.bot_days().await.unwrap();
    assert!(bad.is_empty());
    let b = bot.iter().find(|x| x.date == today).unwrap();
    assert_eq!(
        b.final_usd + b.provisional_usd,
        day.result_usd,
        "запись бота = итог дня в памяти"
    );

    // Все дни: + старый формат (400 закрытых «Closed: …» 01–05.11 и остановленная при перезапуске).
    let mut every = deals.clone();
    every.extend(old_format());
    every.push(old_restart(8, 1234));
    let wall = Window {
        from: None,
        to: None,
        now,
    };
    let rows = arb::report::rows::rows(&every, &wall, &dex);
    // Конец старых записей и несколько дней — tests/report_old_format.rs.
    let pr = rows.iter().find(|x| x.id.starts_with('p')).unwrap();
    let (res, ret) = pr.plan_parts.unwrap();
    assert!(
        res > 0.0 && (ret - 0.15).abs() < 1e-12,
        "запас {res}, возврат {ret}"
    );
    let zr = rows.iter().find(|x| x.id.starts_with('z')).unwrap();
    assert_eq!(
        (zr.invested, zr.fact, zr.costs.get("Газ обменов, USDT")),
        (0.0, Some(-0.0021), Some(&0.0021))
    );

    // Слепок по-настоящему.
    let snap = arb::report::snap::gather(&core).await;
    assert_eq!(snap.problem, None);
    let wl = snap
        .places
        .iter()
        .find(|x| x.name.contains("Кошелёк DEX"))
        .unwrap();
    assert!(wl.error.as_deref().unwrap().contains("DEX не подключена"));
    let tp = snap.places.iter().find(|x| x.name == "В пути").unwrap();
    assert!(
        tp.lines.iter().any(|l| l.note.contains(&tr.id))
            && tp.deals.iter().any(|x| x.contains(&tr.id))
    );
    assert!(
        tp.lines.iter().all(|l| l.doubt),
        "«В пути» — под вопросом, в итог не входит"
    );
    let orphan = snap
        .places
        .iter()
        .find(|x| x.deals.iter().any(|t| t.contains(&ms.id)))
        .unwrap();
    assert!(orphan.error.is_some() && orphan.name.contains("MEXC"));

    let cx = arb::report::ResultsCtx {
        bot,
        prov_days,
        dex,
        snap,
        problems: vec![],
        long_deal_s: Ok(300),
    };
    let bytes = arb::report::results(&every, wall, &cx).expect("книга");
    if let Ok(out) = std::env::var("REPORT_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
    let _ = std::fs::remove_dir_all(&p);
}
