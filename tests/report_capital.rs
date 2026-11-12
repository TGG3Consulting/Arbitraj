//! Файл: tests/report_capital.rs
//! Что проверяет: решение 256 (с дополнением — аудит отчёта) — книга «Сделки, лимиты и капитал» (report/cap_*.rs) на
//! записанных данных:
//! - сделки — 400 сделок учёта бота 01–05.11.2026 (tests/data/report_2026-11-05/deals.csv: старт, длительность, монета,
//!   площадки, вложено, факт, план) как записи учёта ядра (core/deal.rs Deal) с исполненной покупкой;
//! - записи «без потолка» — расчёты ядра на записанных данных CAKE (tests/size_common: стакан BingX ws_spot_2026-10-23,
//!   пул CAKE/USDT V2 — как tests/report_missed.rs), итоги той же функцией, что у экрана (core/uncapped_sum.rs totals).
//!
//! Книга: листы «Сделки», «Упущено из-за потолка», «Итоги», «Лимиты на момент выгрузки», «Слепок по балансам»;
//! заголовки; формулы COUNTIFS / SUMIFS / AVERAGEIFS и формулы массива MAX(IF(…)); без _xlfn, XLOOKUP / FILTER / UNIQUE.
//! Если в среде есть LibreOffice, книга пересчитывается им и числа «Итогов» и столбцов занятости листа «Сделки»
//! сверяются с прямым расчётом по тем же строкам. Деление на части по дням: каждая часть не больше предела, отдаётся
//! сразу (получатель закрыт после первой — сборка останавливается). Периоды — календарные дни UTC, свой интервал.
//! Записывает книгу в REPORT_CAPITAL_OUT (если задан) для просмотра.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::report, arb::core, tests/size_common, tests/cap_common, LibreOffice (необязательно).

mod cap_common;
mod size_common;

use std::collections::HashMap;

use arb::core::deal::{Deal, Step};
use arb::core::size_uncapped::{At, uncapped};
use arb::core::types::{Asset, Direction, VenueId};
use arb::core::uncapped_report::MissedData;
use arb::core::uncapped_sum::totals;
use arb::core::uncapped_types::{BASIS_DECISION, BASIS_SNAPSHOT, Battle, Brief, Episode, missed};
use arb::report::cap_episodes::DayRow;
use arb::report::cap_parts::files;
use arb::report::cap_rows::{DealRow, RowsCtx};
use arb::report::cap_send::{Inputs, capital_data};
use arb::report::cap_xlsx::{File, places, workbook};
use arb::report::rows_time::Window;
use arb::report::sig_period::{Period, parse_range};
use cap_common::{bought, near, num, recalc, unzip};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{CAKE, Rec, d, params};

/// Листы книги.
const SHEETS: [&str; 5] = [
    "Сделки",
    "Упущено из-за потолка",
    "Итоги",
    "Лимиты на момент выгрузки",
    "Слепок по балансам",
];

/// Код площадки deals.csv → id площадки учёта.
fn venue(c: char) -> &'static str {
    match c {
        'B' => "bingx",
        'D' => "bsc_dex",
        'M' => "mexc",
        'K' => "kucoin",
        _ => "gate",
    }
}

/// Сделки учёта из deals.csv (закрыты, вложено > 0, итог есть). Ошибок нет (паника — файл не читается).
fn deals() -> Vec<Deal> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/report_2026-11-05/deals.csv"
    );
    let text = std::fs::read_to_string(p).unwrap();
    let rows = text.lines().filter(|l| !l.is_empty());
    rows.enumerate()
        .map(|(i, l)| {
            let f: Vec<&str> = l.split(',').collect();
            let (md, hm) = f[0].split_once(' ').unwrap();
            let start = DateTime::parse_from_rfc3339(&format!("2026-{md}T{hm}:00Z"))
                .unwrap()
                .with_timezone(&Utc);
            let codes: Vec<char> = f[3].chars().collect();
            let dir = Direction {
                asset: Asset {
                    ticker: f[2].into(),
                    bsc_contract: None,
                },
                buy_on: VenueId(venue(codes[0]).into()),
                sell_on: VenueId(venue(codes[1]).into()),
            };
            let mut x = Deal::new(format!("csv{i:03}"), dir, serde_json::json!({}), d(f[6]));
            x.step = Step::Closed;
            x.invested = d(f[4]);
            x.result_usd = Some(d(f[5]));
            x.started_at = start;
            // Продано на площадке продажи (возврата USDT в записи нет); покупка — исполнена (вложено > 0).
            x.at = x.direction.sell_on.clone();
            bought(&mut x);
            x.updated_at = start + Duration::seconds(f[1].parse().unwrap());
            // Решение 257: конец — запись закрытия в хронологии; сделки 01–05.11 закрыты до 86832aa («Closed: …»,
            // живой текст — docs/notes/usdt-return-close-2026-11-04.md).
            x.timeline.push((
                x.updated_at,
                "Closed: продано (перегон не выгоднее продажи здесь)".into(),
            ));
            x
        })
        .collect()
}

/// Направление CAKE на площадках теста.
fn cake(dr: Dir) -> Direction {
    let (b, s) = match dr {
        Dir::BingxToDex => ("bingx", "bsc_dex"),
        Dir::DexToBingx => ("bsc_dex", "bingx"),
    };
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE),
        },
        buy_on: VenueId(b.into()),
        sell_on: VenueId(s.into()),
    }
}

/// Запись «без потолка» по расчёту ядра на записанных данных (как tests/report_missed.rs). Принимает: данные,
/// направление, бюджет, вердикт, id сделки, начало эпизода, время записи, что ограничило. Ошибок нет.
#[allow(clippy::too_many_arguments)]
async fn brief(
    rec: &Rec,
    dr: Dir,
    budget: &str,
    verdict: &str,
    deal: Option<&str>,
    first: DateTime<Utc>,
    at: DateTime<Utc>,
    limited_by: &str,
) -> Brief {
    let mut c = RecCurve {
        rec,
        dir: dr,
        slip: Decimal::ONE,
    };
    let p = params(rec, dr, d("1000000"));
    let u = uncapped(
        &mut c,
        &p,
        d(budget),
        &LADDER.map(d),
        |pt| pt.profit >= Decimal::ZERO,
        None,
    )
    .await
    .unwrap();
    let zero = At {
        qty: Decimal::ZERO,
        invested: Decimal::ZERO,
        profit: Decimal::ZERO,
    };
    let ab = u.at_budget.at.unwrap_or(zero);
    let opt = At::of(&u.opt);
    Brief {
        at,
        direction: cake(dr),
        battle: Battle {
            verdict: verdict.into(),
            reason: None,
            deal_id: deal.map(String::from),
            dry: false,
            qty: ab.qty,
            invested: ab.invested,
            profit: ab.profit,
            budget: Some(d(budget)),
            plan_budget: None,
            free: None,
            parts: None,
            window_from: None,
        },
        limited_by: limited_by.into(),
        missed: missed(verdict, u.opt_passes, opt.profit, &u.at_budget),
        opt,
        opt_passes: u.opt_passes,
        min_plus: u.min_plus.as_ref().map(At::of),
        at_budget: u.at_budget.clone(),
        levels: u.levels.clone(),
        ep: Episode {
            first_at: first,
            last_at: at,
            count: 3,
            capped_count: 2,
            peak_at: at,
        },
        budget: d(budget),
        basis: BASIS_DECISION.into(),
        decided_at: deal.map(|_| at),
        maxwin: None,
    }
}

/// Потолки теста (как uncapped.ladder_usdt).
const LADDER: [&str; 3] = ["20", "100", "1000"];

/// Данные «Упущено»: 02.11 — эпизод BingX → DEX со сделкой, два эпизода DEX → BingX (один — запись сделки и кандидат
/// с тем же началом), 03.11 — эпизод DEX → BingX, и несравнимая запись сделки (decision_snapshot). Ошибок нет.
async fn missed_data() -> MissedData {
    let rec = Rec::load("report-capital").await;
    let t = |day: u32, h: u32, m: u32| Utc.with_ymd_and_hms(2026, 11, day, h, m, 0).unwrap();
    let (bd, db) = (Dir::BingxToDex, Dir::DexToBingx);
    // Кандидаты BingX → DEX на этих данных без упущенного (скрываются, как у экрана) — кандидаты здесь DEX → BingX.
    let mut briefs = vec![
        brief(
            &rec,
            db,
            "20",
            "reject",
            None,
            t(3, 9, 0),
            t(3, 9, 1),
            "free",
        )
        .await,
        brief(
            &rec,
            db,
            "20",
            "reject",
            None,
            t(2, 8, 0),
            t(2, 8, 2),
            "optimum",
        )
        .await,
        brief(
            &rec,
            db,
            "20",
            "reject",
            None,
            t(2, 7, 0),
            t(2, 7, 3),
            "liquidity",
        )
        .await,
        brief(
            &rec,
            db,
            "20",
            "deal",
            Some("dA"),
            t(2, 7, 0),
            t(2, 7, 0),
            "per_trade",
        )
        .await,
        brief(
            &rec,
            bd,
            "20",
            "deal",
            Some("dB"),
            t(2, 6, 0),
            t(2, 6, 0),
            "per_trade",
        )
        .await,
    ];
    let mut old = brief(
        &rec,
        db,
        "20",
        "deal",
        Some("dS"),
        t(2, 5, 0),
        t(2, 5, 0),
        "per_coin",
    )
    .await;
    old.basis = BASIS_SNAPSHOT.into();
    briefs.push(old);
    let ladder: Vec<Decimal> = LADDER.map(d).to_vec();
    MissedData {
        totals: totals(briefs, &ladder, |_| None),
        ladder_error: None,
        off: None,
        labels: HashMap::new(),
        maxwin: arb::core::maxwin_view::Ctx {
            now: Utc::now(),
            gap: Err("не задана".into()),
            min_n: Err("не задана".into()),
            times: Default::default(),
        },
    }
}

/// Строки таблицы «Итогов», начиная с заголовка `title`: первая ячейка → ячейки (до «Итого…» / пустой строки). Шапка —
/// через `skip` строк после заголовка. Ошибок нет (паника — таблицы нет).
fn table(
    rows: &[Vec<String>],
    title: &str,
    skip: usize,
) -> (Vec<String>, HashMap<String, Vec<String>>) {
    let i = rows.iter().position(|r| r[0].starts_with(title)).unwrap();
    let hdr = rows[i + skip].clone();
    let mut out = HashMap::new();
    for r in &rows[i + skip + 1..] {
        if r[0].starts_with("Итого") || r[0].is_empty() {
            break;
        }
        out.insert(r[0].clone(), r.clone());
    }
    (hdr, out)
}

/// Строка «Итогов», первая ячейка которой начинается с `label`, после заголовка таблицы `title`. Ошибок нет.
fn line<'a>(rows: &'a [Vec<String>], title: &str, label: &str) -> &'a Vec<String> {
    let i = rows.iter().position(|r| r[0].starts_with(title)).unwrap();
    rows[i..].iter().find(|r| r[0].starts_with(label)).unwrap()
}

/// Прямой расчёт занятости в момент начала `x` (то же правило, что формулы листа): (сумма вложенного, число сделок).
/// `place` — только та же площадка денег покупки. Ошибок нет.
pub fn busy(rows: &[DealRow], x: &DealRow, place: bool) -> (f64, usize) {
    let v: Vec<&DealRow> = rows
        .iter()
        .filter(|o| o.hold.counted && o.start <= x.start)
        .filter(|o| o.hold.until.is_none_or(|u| u >= x.start))
        .filter(|o| !place || o.hold.place == x.hold.place)
        .collect();
    (v.iter().map(|o| o.invested).sum(), v.len())
}

/// Сверка столбцов занятости листа «Сделки» с прямым расчётом. Паника — не совпало.
pub fn check_busy(sd: &[Vec<String>], rows: &[DealRow]) {
    for (i, r) in rows.iter().enumerate() {
        let c = &sd[i + 1];
        if !r.hold.counted {
            assert_eq!(
                (c[26].as_str(), c[27].as_str()),
                ("", ""),
                "вне расчёта — пусто"
            );
            continue;
        }
        let (s, n) = busy(rows, r, true);
        near(num(&c[26]).unwrap(), s, "занято на площадке");
        near(num(&c[27]).unwrap(), n as f64, "открыто на площадке");
        near(num(&c[28]).unwrap(), busy(rows, r, false).0, "занято всего");
    }
}

/// Сверка «Итогов» с прямым расчётом по строкам. Ошибок нет (паника — не совпало).
pub fn check_summary(s: &[Vec<String>], deals: &[DealRow], days: &[DayRow], ladder: &[f64]) {
    let dirs: Vec<&str> = {
        let mut v: Vec<&str> = (deals.iter().map(|r| r.dir.as_str()))
            .chain(days.iter().map(|r| r.dir.as_str()))
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let (_, t1) = table(s, "1.", 1);
    let (_, t2) = table(s, "2.", 1);
    let (h3, t3) = table(s, "3.", 1);
    let filled = h3.iter().filter(|c| !c.is_empty()).count();
    assert_eq!(
        filled,
        2 + ladder.len() + ladder.len().saturating_sub(1),
        "шапка таблицы 3"
    );
    let mut total = 0.0;
    for dir in dirs {
        let ds: Vec<&DayRow> = days.iter().filter(|r| r.dir == dir).collect();
        let sum = |f: &dyn Fn(&DayRow) -> f64| ds.iter().map(|r| f(r)).sum::<f64>();
        let r1 = &t1[dir];
        near(
            num(&r1[1]).unwrap(),
            sum(&|r| r.episodes as f64),
            "эпизодов",
        );
        for k in 0..3 {
            near(
                num(&r1[2 + k]).unwrap(),
                sum(&|r| r.limits[k] as f64),
                "ограничил",
            );
        }
        let r3 = &t3[dir];
        near(num(&r3[1]).unwrap(), sum(&|r| r.at_budget), "наш бюджет");
        for k in 0..ladder.len() {
            let v = sum(&|r| r.caps[k].unwrap_or(0.0));
            near(num(&r3[2 + k]).unwrap(), v, "потолок");
        }
        let dd: Vec<&DealRow> = deals
            .iter()
            .filter(|r| r.dir == dir && r.in_total)
            .collect();
        let r2 = &t2[dir];
        let closed: Vec<&&DealRow> = dd.iter().filter(|r| r.fact.is_some()).collect();
        let fact: f64 = closed.iter().map(|r| r.fact.unwrap()).sum();
        let est: f64 = dd.iter().filter_map(|r| r.estimate).sum();
        let plan: f64 = closed.iter().map(|r| r.plan).sum();
        let neg: Vec<f64> = closed
            .iter()
            .map(|r| r.fact.unwrap())
            .filter(|v| *v < 0.0)
            .collect();
        near(num(&r2[1]).unwrap(), dd.len() as f64, "сделок в итоге");
        near(num(&r2[2]).unwrap(), closed.len() as f64, "закрыты");
        near(num(&r2[3]).unwrap(), plan, "план закрытых");
        near(num(&r2[4]).unwrap(), fact, "факт");
        near(num(&r2[5]).unwrap(), est, "оценка");
        near(num(&r2[6]).unwrap(), fact + est, "факт + оценка");
        if plan != 0.0 {
            near(num(&r2[7]).unwrap(), fact / plan, "факт / план");
        }
        near(
            num(&r2[8]).unwrap(),
            dd.iter().map(|r| r.spent).sum(),
            "затраты",
        );
        near(
            num(&r2[9]).unwrap(),
            dd.iter().map(|r| r.ret_cost).sum(),
            "затраты возврата",
        );
        near(num(&r2[10]).unwrap(), neg.len() as f64, "минусовых");
        near(num(&r2[11]).unwrap(), neg.iter().sum(), "сумма минусовых");
        total += fact + est;
    }
    near(
        num(&line(s, "2.", "Итого")[6]).unwrap(),
        total,
        "итог периода",
    );
    check_balance(s, deals, ladder);
}

/// Таблицы 4 и 5 (по площадке денег покупки) против прямого расчёта. Паника — не совпало.
fn check_balance(s: &[Vec<String>], deals: &[DealRow], ladder: &[f64]) {
    let (_, t4) = table(s, "4.", 1);
    let (_, t5) = table(s, "5.", 2);
    let mut sum_max = 0.0;
    for p in places_of(deals) {
        let inp: Vec<&DealRow> = deals.iter().filter(|r| r.hold.place == p).collect();
        let counted: Vec<&&DealRow> = inp.iter().filter(|r| r.hold.counted).collect();
        let max_s = counted
            .iter()
            .map(|r| busy(deals, r, true).0)
            .fold(0.0, f64::max);
        let max_n = counted
            .iter()
            .map(|r| busy(deals, r, true).1)
            .max()
            .unwrap_or(0);
        let r4 = &t4[&p];
        near(
            num(&r4[1]).unwrap(),
            counted.len() as f64,
            "сделок в расчёте",
        );
        if counted.is_empty() {
            assert_eq!(r4[2], "");
        } else {
            near(num(&r4[2]).unwrap(), max_s, "максимум занятого");
            near(num(&r4[3]).unwrap(), max_n as f64, "максимум открытых");
        }
        let out = inp.iter().filter(|r| r.bought && !r.hold.counted).count();
        near(num(&r4[5]).unwrap(), out as f64, "вне расчёта");
        sum_max += max_s;
        let r5 = &t5[&p];
        near(
            num(&r5[1]).unwrap(),
            max_n as f64,
            "модель: максимум открытых",
        );
        for (k, c) in ladder.iter().enumerate() {
            near(num(&r5[2 + k]).unwrap(), c * max_n as f64, "модель: баланс");
        }
    }
    near(
        num(&line(s, "4.", "Итого")[2]).unwrap(),
        sum_max,
        "сумма максимумов",
    );
    let all = deals
        .iter()
        .filter(|r| r.hold.counted)
        .map(|r| busy(deals, r, false).0)
        .fold(0.0, f64::max);
    near(
        num(&line(s, "4.", "Максимум одновременно")[2]).unwrap(),
        all,
        "всего вместе",
    );
}

/// Площадки денег покупки строк с покупкой. Ошибок нет.
fn places_of(deals: &[DealRow]) -> Vec<String> {
    let mut v: Vec<String> = deals
        .iter()
        .filter(|r| r.bought)
        .map(|r| r.hold.place.clone())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Входные данные без ядра: площадка DEX теста, слепок не снимался (причина), лимитов нет.
fn inputs() -> Inputs {
    Inputs {
        rows: RowsCtx {
            dex: ["bsc_dex".to_string()].into(),
            ..Default::default()
        },
        limits: vec![],
        snap: arb::report::snap::Snapshot {
            places: vec![],
            problem: Some("тест без ядра: слепок не снимается".into()),
        },
        keep_days: Ok(30),
    }
}

#[tokio::test]
async fn capital_workbook_from_recorded_data() {
    let all = deals();
    let m = missed_data().await;
    let now = Utc.with_ymd_and_hms(2026, 11, 5, 12, 20, 0).unwrap();
    let w = Window {
        from: None,
        to: None,
        now,
    };
    let data = capital_data(&all, &m, &w, "за всё время", &inputs());
    assert_eq!(data.deals.len(), 400);
    assert!(
        data.deals
            .iter()
            .all(|r| r.in_total && r.bought && r.hold.counted)
    );
    assert!(
        data.deals.windows(2).all(|w| w[0].start <= w[1].start),
        "старые сверху"
    );
    let key = |r: &DayRow| {
        (
            r.day.format("%d.%m").to_string(),
            r.dir.clone(),
            r.episodes,
            r.with_deal,
        )
    };
    let got: Vec<_> = data.days.iter().map(key).collect();
    assert_eq!(
        got,
        vec![
            ("02.11".into(), "BingX → DEX".into(), 1, 1),
            ("02.11".into(), "DEX → BingX".into(), 2, 1),
            ("03.11".into(), "DEX → BingX".into(), 1, 0),
        ]
    );
    assert!(
        data.notes.iter().any(|n| n.contains("не входят: 1")),
        "{:?}",
        data.notes
    );
    assert!(
        data.notes
            .iter()
            .any(|n| n.contains("uncapped.keep_days = 30")),
        "срок хранения"
    );
    assert!(
        data.notes
            .iter()
            .any(|n| n.contains("с первой сделки в учёте")),
        "окно"
    );
    let lim: u64 = data.days.iter().map(|r| r.limits.iter().sum::<u64>()).sum();
    assert_eq!(lim, 4, "по одному «что ограничило» на эпизод");
    assert_eq!(
        places(&data),
        [
            "BingX",
            "DEX (кошелёк в сделке не записан)",
            "Gate",
            "KuCoin",
            "MEXC"
        ]
        .map(String::from)
    );

    let bytes = workbook(&data, None).expect("книга");
    if let Ok(out) = std::env::var("REPORT_CAPITAL_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
    let x = unzip(&bytes);
    for s in SHEETS {
        assert!(
            x["xl/workbook.xml"].contains(&format!("name=\"{s}\"")),
            "лист {s}"
        );
    }
    let strings = &x["xl/sharedStrings.xml"];
    for h in arb::report::cap_deals::HDR {
        assert!(strings.contains(h), "заголовок «{h}»");
    }
    assert!(strings.contains("Потолок модели uncapped.ladder_usdt, USDT →"));
    assert!(
        !strings.contains("Лимит на сделку, USDT →"),
        "потолок модели не выдаётся за лимит"
    );
    let (s1, s3) = (
        &x["xl/worksheets/sheet1.xml"],
        &x["xl/worksheets/sheet3.xml"],
    );
    assert!(
        s1.contains("<f>ROUND((J2-I2)*86400,0)</f>"),
        "длительность — формулой"
    );
    assert!(
        s1.contains("SUMIFS('Сделки'!$M$2:$M$401,'Сделки'!$W$2:$W$401,W2,"),
        "занято на площадке"
    );
    for f in [
        "COUNTIFS(",
        "SUMIFS(",
        "AVERAGEIFS(",
        "MAX(IF(",
        "'Упущено из-за потолка'!$C$2:$C$4",
    ] {
        assert!(s3.contains(f), "«Итоги»: {f}");
    }
    for f in ["_xlfn", "XLOOKUP", "FILTER(", "UNIQUE("] {
        assert!(x.iter().all(|(_, t)| !t.contains(f)), "без {f}");
    }

    match recalc(&bytes, &SHEETS) {
        Some(lo) => {
            assert_eq!(lo["Сделки"].len(), 401);
            check_busy(&lo["Сделки"], &data.deals);
            let ladder: Vec<f64> = LADDER.iter().map(|c| c.parse().unwrap()).collect();
            check_summary(&lo["Итоги"], &data.deals, &data.days, &ladder);
            for (name, t) in &lo {
                assert!(
                    t.iter()
                        .flatten()
                        .all(|c| !c.starts_with('#') && !c.starts_with("Err:")),
                    "ошибка формулы на листе {name}"
                );
            }
        }
        None => eprintln!("LibreOffice нет — пересчёт формул пропущен, проверены строки формул"),
    }

    // Части: предел меньше книги — несколько файлов, каждый не больше предела, отдаются по порядку _part1, _part2…
    let max = bytes.len() * 9 / 10;
    let mut got: Vec<File> = vec![];
    let n = files(
        &data,
        max,
        "arb_capital_2026-11-05",
        "Отчёт",
        &mut |f| {
            got.push(f);
            true
        },
    )
    .unwrap();
    assert!(n > 1 && n == got.len());
    assert!(got.iter().all(|f| f.bytes.len() <= max));
    for (k, f) in got.iter().enumerate() {
        assert_eq!(f.name, format!("arb_capital_2026-11-05_part{}.xlsx", k + 1));
        assert!(f.caption.contains(&format!("часть {}: дни", k + 1)));
    }
    // Часть отдаётся сразу: получатель закрыт после первой — дальше не собирается.
    let mut calls = 0;
    let sent = files(&data, max, "p", "c", &mut |_| {
        calls += 1;
        false
    })
    .unwrap();
    assert_eq!((sent, calls), (0, 1));
    let err = files(&data, 1000, "p", "c", &mut |_| true).unwrap_err();
    assert!(err.contains("больше предела"), "{err}");
}

#[test]
fn periods_calendar_and_custom_interval() {
    let p = parse_range(" 01.11.26 – 07.11.26 ").unwrap();
    let day = |m, dd| NaiveDate::from_ymd_opt(2026, m, dd).unwrap();
    assert_eq!(p, Period::Range(day(11, 1), day(11, 7)));
    let now = Utc.with_ymd_and_hms(2026, 11, 11, 15, 30, 0).unwrap();
    let w = p.calendar(now);
    assert_eq!(
        w.from.unwrap(),
        Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap()
    );
    assert_eq!(
        w.to.unwrap(),
        Utc.with_ymd_and_hms(2026, 11, 8, 0, 0, 0).unwrap()
    );
    assert!(parse_range("01.11.2026-07.11.26").is_err());
    assert!(
        parse_range("07.11.26-01.11.26")
            .unwrap_err()
            .contains("позже")
    );
    assert!(
        parse_range("31.02.26-01.03.26")
            .unwrap_err()
            .contains("такой даты нет")
    );
    // Календарные дни UTC, как «Итоги сделок» (rows_time.rs calendar_from).
    let midnight = |dd| Utc.with_ymd_and_hms(2026, 11, dd, 0, 0, 0).unwrap();
    assert_eq!(Period::Today.calendar(now).from.unwrap(), midnight(11));
    assert_eq!(Period::Days(3).calendar(now).from.unwrap(), midnight(9));
    assert_eq!(Period::Days(7).calendar(now).from.unwrap(), midnight(5));
    assert_eq!(Period::Days(3).calendar(now).to, None);
    let all = Period::All.calendar(now);
    assert_eq!((all.from, all.to, all.now), (None, None, now));
    assert!(
        Period::Days(3)
            .calendar(now)
            .text()
            .contains("2026-11-09 00:00:00")
    );
}
