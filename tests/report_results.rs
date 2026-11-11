//! Файл: tests/report_results.rs
//! Что проверяет: решение 257 — отчёт «Итоги сделок» на настоящей SQLite (ядро без площадок в реестре, как тесты учёта):
//! 400 записанных сделок отчёта 05.11 (tests/data/report_2026-11-05/deals.csv: монета, площадки, вложено, факт, план)
//! закрываются тем же вызовом, что у бота (ledger::finish) — итог дня бота считает core/daily.rs; плюс остановленная
//! (оценка), «Беру на себя» (цены нет — худший случай), вложение 0 с газом (живой газ кошелька DEX 0.0021 USDT, тест
//! 236), возврат USDT «вернулись» после закрытия (живой случай MEXC: ушло 27.6, зачислено 27.59 — поправка −0.01 в день
//! поправки) и сделка до решения 257 с оценкой вчерашнего дня (коррекция «факт − оценка» сегодня).
//! Проверки: начисления «Учёта дня» за сегодня = итогу дня бота (DayView и запись daily_result), кроме строки
//! «восстановлено» сделки до 257 — она ровно равна разнице; сделки в отчёте — с оценкой отдельно от факта, вложение 0 —
//! в отчёте; длительность — до записи о закрытии, не до updated_at. Книга пишется в REPORT_OUT (если задан) для
//! пересчёта LibreOffice и сверки в Python.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::report, arb::settings, arb::storage, arb::notify, arb::secrets. Моков нет.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::types::{Asset, CostKind, Direction, VenueId};
use arb::core::usdt_return_state::UsdtReturn;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::report::rows_time::{Window, calendar_from};
use arb::report::snap::{Line, Place};
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::json;
use tokio::sync::broadcast;

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("arb-res-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Ядро на базе в папке.
async fn start(p: &Path) -> Core {
    let db = Db::open(&p.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(p.join("secrets.enc")),
        journal: journal.clone(),
    };
    Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: p.to_path_buf(),
    })
    .await
}

/// Сделки из deals.csv: (id, сделка с вложением и выручкой = вложено + факт, затрат 0 — в записи их нет).
fn recorded() -> Vec<Deal> {
    let v = |c: char| match c {
        'B' => "bingx",
        'D' => "bsc_dex",
        'M' => "mexc",
        'K' => "kucoin",
        _ => "gate",
    };
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/report_2026-11-05/deals.csv"
    );
    let text = std::fs::read_to_string(p).unwrap();
    text.lines()
        .filter(|l| !l.is_empty())
        .enumerate()
        .map(|(k, l)| {
            let f: Vec<&str> = l.split(',').collect();
            let codes: Vec<char> = f[3].chars().collect();
            let dir = Direction {
                asset: Asset {
                    ticker: f[2].into(),
                    bsc_contract: None,
                },
                buy_on: VenueId(v(codes[0]).into()),
                sell_on: VenueId(v(codes[1]).into()),
            };
            let mut x = Deal::new(format!("r{k:03}{}", f[2]), dir, json!(null), d(f[6]));
            let (md, hm) = f[0].split_once(' ').unwrap();
            let s = format!("2026-{md}T{hm}:00Z");
            x.started_at = chrono::DateTime::parse_from_rfc3339(&s)
                .unwrap()
                .with_timezone(&Utc);
            x.invested = d(f[4]);
            x.proceeds = d(f[4]) + d(f[5]);
            x
        })
        .collect()
}

#[tokio::test]
async fn day_matches_bot_and_book_builds() {
    let p = dir("day");
    // Вчера: оценка −0.26 по сделке «old1» (запись daily_result, как у бота до решения 257).
    {
        let db = Db::open(&p.join("t.sqlite")).unwrap();
        let y = (Utc::now().date_naive() - chrono::Duration::days(1)).to_string();
        let body = json!({"final_usd": "-0.26", "provisional": {"old1": ["-0.26", y]}}).to_string();
        db.call(move |c| {
            c.execute(
                "INSERT INTO daily_result (day, body, updated_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![y, body, "t"],
            )
            .map(|_| ())
        })
        .await
        .unwrap();
    }
    let core = start(&p).await;
    let mut all = recorded();
    let fresh = recorded();
    let n = all.len();
    for x in all.iter_mut() {
        arb::core::ledger::finish(&core, x, Step::Closed, "запись 05.11").await;
    }
    // Остановленная без монет (оценка = известные суммы) и «Беру на себя» с монетами (цены нет — худший случай).
    let mut h = fresh[0].clone();
    (h.id, h.proceeds) = ("h1".into(), Decimal::ZERO);
    arb::core::ledger::finish(&core, &mut h, Step::Halted, "тест: остановлена").await;
    let mut t = fresh[1].clone();
    (t.id, t.proceeds, t.qty) = ("t1".into(), Decimal::ZERO, d("1"));
    arb::core::ledger::finish(&core, &mut t, Step::TakenOver, "тест: Беру на себя").await;
    // Вложение 0, газ 0.0021 USDT (живой газ кошелька DEX, тест 236).
    let mut z = fresh[2].clone();
    (z.id, z.invested, z.proceeds) = ("z1".into(), Decimal::ZERO, Decimal::ZERO);
    arb::core::deal_book::spend(&mut z, CostKind::Gas, d("0.0021"));
    arb::core::ledger::finish(&core, &mut z, Step::Closed, "тест: покупка не прошла").await;
    // Возврат USDT MEXC → DEX «вернулся» после закрытия: ушло 27.6, зачислено 27.59.
    let mut r = fresh[3].clone();
    r.id = "u1".into();
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
    arb::core::usdt_return_state::set(&core, "u1", done, "вернулись").await;
    // Сделка до решения 257 с оценкой вчера: закрыта сегодня — коррекция «факт − оценка» в итоге дня сегодня.
    let mut o = fresh[4].clone();
    o.id = "old1".into();
    arb::core::ledger::finish(&core, &mut o, Step::Closed, "тест: разбор").await;

    let deals = core.deals();
    let u = deals.iter().find(|x| x.id == "u1").unwrap();
    assert_eq!(u.return_cost_usd, Some(d("0.01")));
    assert_eq!(u.day_book.last().map(|e| e.amount), Some(d("-0.01")));
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
        ledger
            .iter()
            .filter(|x| x.date == today && x.fact == fact && booked.is_none_or(|b| x.booked == b))
            .map(|x| x.amount)
            .sum()
    };
    let restored = sum(true, Some(false));
    let o_res = deals
        .iter()
        .find(|x| x.id == "old1")
        .unwrap()
        .result_usd
        .unwrap();
    // Остаток сделки до 257 = её вчерашняя оценка (−0.26): бот положил её во вчерашний день.
    assert!(
        (restored - (-0.26)).abs() < 1e-9,
        "восстановлено {restored}, итог {o_res}"
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

    let rows = arb::report::rows::rows(&deals, &w, &dex);
    assert_eq!(
        rows.len(),
        n + 5,
        "400 записанных + остановленная, «Беру на себя», вложение 0, возврат, old1"
    );
    let hr = rows.iter().find(|x| x.id == "h1").unwrap();
    assert!(hr.fact.is_none() && hr.estimate.is_some() && hr.status.contains("оценка"));
    let zr = rows.iter().find(|x| x.id == "z1").unwrap();
    assert_eq!((zr.invested, zr.fact), (0.0, Some(-0.0021)));
    assert_eq!(zr.costs.get("Газ обменов, USDT"), Some(&0.0021));
    let ur = rows.iter().find(|x| x.id == "u1").unwrap();
    assert!(
        ur.duration_s.is_some() && ur.end.unwrap() < u.updated_at,
        "конец — закрытие, не последняя запись"
    );

    // Слепок: ядро без площадок — пусто; блок из записанных чисел (зачислено 27.59 USDT на MEXC и CAKE без цены).
    let mut snap = arb::report::snap::gather(&core).await;
    assert!(snap.is_empty());
    snap.push(Place {
        name: "MEXC".into(),
        at: Some(Utc.with_ymd_and_hms(2026, 11, 9, 12, 0, 0).unwrap()),
        error: None,
        lines: vec![
            Line {
                account: "Спот".into(),
                asset: "USDT".into(),
                qty: 27.59,
                price: Ok(1.0),
            },
            Line {
                account: "Спот".into(),
                asset: "CAKE".into(),
                qty: 24.7,
                price: Err("нет стакана".into()),
            },
        ],
        deals: vec!["На момент отчёта активная сделка (пример строки)".into()],
    });
    let cx = arb::report::ResultsCtx {
        bot,
        prov_days,
        dex,
        snap,
        problems: vec![],
    };
    let bytes = arb::report::results(&deals, w, &cx).expect("книга");
    assert!(bytes.len() > 10_000);
    if let Ok(out) = std::env::var("REPORT_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
    let _ = std::fs::remove_dir_all(&p);
}
