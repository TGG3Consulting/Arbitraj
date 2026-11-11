//! Файл: tests/report_capital.rs
//! Что проверяет: решение 256 — отчёт «Сделки, лимиты и капитал» (report/cap_*.rs) на записанных данных:
//! - сделки — 400 сделок учёта бота 01–05.11.2026 (tests/data/report_2026-11-05/deals.csv: старт, длительность, монета,
//!   площадки, вложено, факт, план — как в tests/report_results.rs) как записи учёта ядра (core/deal.rs Deal);
//! - записи «без потолка» — расчёты ядра на записанных данных CAKE (tests/size_common: стакан BingX ws_spot_2026-10-23,
//!   пул CAKE/USDT V2 — как tests/report_missed.rs), итоги той же функцией, что у экрана (core/uncapped_sum.rs totals).
//!
//! Книга: листы «Сделки», «Упущено из-за потолка», «Итоги»; заголовки столбцов; формулы COUNTIFS / SUMIFS /
//! _xlfn.MAXIFS / AVERAGEIFS на месте, без XLOOKUP / FILTER / UNIQUE. Если в среде есть LibreOffice (soffice), книга
//! пересчитывается им (профиль с «пересчитывать всегда при загрузке» — OOXMLRecalcMode = 0) и выгружается в CSV по
//! листам; числа «Итогов» и столбца «Одновременно открыто» сверяются с прямым расчётом по тем же строкам. Нет
//! LibreOffice — проверяются строки формул и диапазоны. Деление на части по дням при малом пределе — каждая часть не
//! больше предела, дни частей идут подряд. Разбор своего интервала дд.мм.гг.
//! Записывает книгу в REPORT_CAPITAL_OUT (если задан) для просмотра.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::report, arb::core (deal, size_uncapped, uncapped_types, uncapped_sum, uncapped_report),
//! tests/size_common, flate2, LibreOffice (необязательно).

mod size_common;

use std::collections::HashMap;
use std::io::Read;
use std::process::Command;

use arb::core::deal::{Deal, Step};
use arb::core::size_uncapped::{At, uncapped};
use arb::core::types::{Asset, Direction, VenueId};
use arb::core::uncapped_report::MissedData;
use arb::core::uncapped_sum::totals;
use arb::core::uncapped_types::{BASIS_DECISION, BASIS_SNAPSHOT, Battle, Brief, Episode, missed};
use arb::report::cap_episodes::DayRow;
use arb::report::cap_rows::DealRow;
use arb::report::cap_send::capital_data;
use arb::report::cap_xlsx::{File, files, workbook};
use arb::report::sig_period::{Period, parse_range};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{CAKE, Rec, d, params};

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

/// Файлы xlsx (zip) по имени: центральный каталог, deflate или без сжатия (как tests/report_missed.rs).
fn unzip(b: &[u8]) -> HashMap<String, String> {
    let u16le = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as usize;
    let u32le = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
    let eocd = (0..b.len() - 3)
        .rev()
        .find(|&i| b[i..i + 4] == [0x50, 0x4b, 5, 6])
        .unwrap();
    let (n, mut p) = (u16le(eocd + 10), u32le(eocd + 16));
    let mut out = HashMap::new();
    for _ in 0..n {
        let (method, csize) = (u16le(p + 10), u32le(p + 20));
        let (nl, el, cl) = (u16le(p + 28), u16le(p + 30), u16le(p + 32));
        let local = u32le(p + 42);
        let name = String::from_utf8(b[p + 46..p + 46 + nl].to_vec()).unwrap();
        let start = local + 30 + u16le(local + 26) + u16le(local + 28);
        let raw = &b[start..start + csize];
        let mut s = String::new();
        if method == 8 {
            flate2::read::DeflateDecoder::new(raw)
                .read_to_string(&mut s)
                .unwrap();
        } else {
            s = String::from_utf8(raw.to_vec()).unwrap();
        }
        out.insert(name, s);
        p += 46 + nl + el + cl;
    }
    out
}

/// Разбор CSV LibreOffice (запятая, кавычки "…" с удвоением). Ошибок нет.
fn csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = vec![];
    let (mut row, mut cell, mut q) = (vec![], String::new(), false);
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match (c, q) {
            ('"', true) if it.peek() == Some(&'"') => {
                cell.push('"');
                it.next();
            }
            ('"', _) => q = !q,
            (',', false) => row.push(std::mem::take(&mut cell)),
            ('\n', false) => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            ('\r', false) => {}
            _ => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// Пересчёт книги LibreOffice и CSV листов по имени листа; None — LibreOffice в среде нет. Паника — сбой пересчёта.
fn recalc(bytes: &[u8]) -> Option<HashMap<String, Vec<Vec<String>>>> {
    Command::new("soffice").arg("--version").output().ok()?;
    let dir = std::env::temp_dir().join(format!("arb-cap-lo-{}", std::process::id()));
    let prof = dir.join("prof/user");
    std::fs::create_dir_all(&prof).unwrap();
    // OOXMLRecalcMode 0 — «всегда пересчитывать» при загрузке xlsx (настройка LibreOffice Calc).
    std::fs::write(
        prof.join("registrymodifications.xcu"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<item oor:path="/org.openoffice.Office.Calc/Formula/Load"><prop oor:name="OOXMLRecalcMode" oor:op="fuse"><value>0</value></prop></item>
</oor:items>"#,
    )
    .unwrap();
    let book = dir.join("cap.xlsx");
    std::fs::write(&book, bytes).unwrap();
    let st = Command::new("soffice")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            dir.join("prof").display()
        ))
        .args(["--headless", "--calc", "--convert-to"])
        .arg("csv:Text - txt - csv (StarCalc):44,34,76,1,,0,false,true,false,false,false,-1")
        .arg("--outdir")
        .arg(&dir)
        .arg(&book)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let mut out = HashMap::new();
    for s in ["Сделки", "Упущено из-за потолка", "Итоги"] {
        let t = std::fs::read_to_string(dir.join(format!("cap-{s}.csv"))).unwrap();
        out.insert(s.to_string(), csv(&t));
    }
    let _ = std::fs::remove_dir_all(&dir);
    Some(out)
}

/// Число из ячейки CSV (пусто — None; «12.5%» — 0.125: процентный формат LibreOffice выгружает со знаком). Ошибок нет.
fn num(s: &str) -> Option<f64> {
    let t = s.trim();
    match t.strip_suffix('%') {
        Some(p) => p.parse::<f64>().ok().map(|v| v / 100.0),
        None => t.parse().ok(),
    }
}

/// Сравнение чисел с допуском. Паника — не совпало.
fn near(a: f64, b: f64, what: &str) {
    assert!((a - b).abs() < 1e-6 * (1.0 + b.abs()), "{what}: {a} ≠ {b}");
}

/// Строки таблицы «Итогов», начиная с заголовка `title`: направление → ячейки (без «Итого»). Шапка — через
/// `skip` строк после заголовка. Ошибок нет (паника — таблицы нет).
fn table(
    rows: &[Vec<String>],
    title: &str,
    skip: usize,
) -> (Vec<String>, HashMap<String, Vec<String>>) {
    let i = rows.iter().position(|r| r[0].starts_with(title)).unwrap();
    let hdr = rows[i + skip].clone();
    let mut out = HashMap::new();
    for r in &rows[i + skip + 1..] {
        if r[0] == "Итого" || r[0].is_empty() {
            break;
        }
        out.insert(r[0].clone(), r.clone());
    }
    (hdr, out)
}

/// Прямой расчёт «одновременно открыто» (то же правило, что формула листа). Ошибок нет.
fn concurrent(rows: &[DealRow], x: &DealRow) -> usize {
    rows.iter()
        .filter(|o| o.bought && o.dir == x.dir && o.start <= x.start)
        .filter(|o| o.end.is_none_or(|e| e >= x.start))
        .count()
}

/// Сверка «Итогов» с прямым расчётом по строкам. Ошибок нет (паника — не совпало).
fn check_summary(lo: &HashMap<String, Vec<Vec<String>>>, deals: &[DealRow], days: &[DayRow]) {
    let s = &lo["Итоги"];
    let dirs: Vec<&str> = {
        let mut v: Vec<&str> = (deals.iter().map(|r| r.dir.as_str()))
            .chain(days.iter().map(|r| r.dir.as_str()))
            .collect();
        v.sort();
        v.dedup();
        v
    };
    // 1. Что ограничивает объём.
    let (_, t1) = table(s, "1.", 1);
    // 2. Модель против факта.
    let (_, t2) = table(s, "2.", 1);
    // 3. Лимиты.
    let (h3, t3) = table(s, "3.", 1);
    let filled = h3.iter().filter(|c| !c.is_empty()).count();
    assert_eq!(
        filled,
        1 + 1 + 3 + 2,
        "направление, наш бюджет, 3 потолка, 2 прибавки"
    );
    // 4. Баланс: заголовок, строка лимитов, шапка.
    let (_, t4) = table(s, "4.", 2);
    for dir in dirs {
        let ds: Vec<&DayRow> = days.iter().filter(|r| r.dir == dir).collect();
        let sum = |f: &dyn Fn(&DayRow) -> f64| ds.iter().map(|r| f(r)).sum::<f64>();
        let r1 = &t1[dir];
        near(
            num(&r1[1]).unwrap(),
            sum(&|r| r.episodes as f64),
            "эпизодов",
        );
        near(
            num(&r1[2]).unwrap(),
            sum(&|r| r.limits[0] as f64),
            "выгодный объём",
        );
        near(
            num(&r1[3]).unwrap(),
            sum(&|r| r.limits[1] as f64),
            "лимит на сделку",
        );
        near(
            num(&r1[4]).unwrap(),
            sum(&|r| r.limits[2] as f64),
            "свободный баланс",
        );
        let r3 = &t3[dir];
        near(num(&r3[1]).unwrap(), sum(&|r| r.at_budget), "наш бюджет");
        for k in 0..3 {
            let v = sum(&|r| r.caps[k].unwrap_or(0.0));
            near(num(&r3[2 + k]).unwrap(), v, "потолок");
        }
        for k in 1..3 {
            let inc = num(&r3[2 + k]).unwrap() - num(&r3[1 + k]).unwrap();
            near(num(&r3[4 + k]).unwrap(), inc, "прибавка");
        }
        let dd: Vec<&DealRow> = (deals.iter())
            .filter(|r| r.dir == dir && r.bought && r.fact.is_some())
            .collect();
        let r2 = &t2[dir];
        let plan: f64 = dd.iter().map(|r| r.plan).sum();
        let fact: f64 = dd.iter().map(|r| r.fact.unwrap()).sum();
        let neg: Vec<f64> = dd
            .iter()
            .filter_map(|r| r.fact)
            .filter(|v| *v < 0.0)
            .collect();
        near(num(&r2[1]).unwrap(), dd.len() as f64, "сделок");
        near(num(&r2[2]).unwrap(), plan, "план");
        near(num(&r2[3]).unwrap(), fact, "факт");
        if plan != 0.0 {
            near(num(&r2[4]).unwrap(), fact / plan, "факт/план");
        }
        near(num(&r2[5]).unwrap(), neg.len() as f64, "минусовых");
        near(num(&r2[6]).unwrap(), neg.iter().sum(), "сумма минусовых");
        let bd: Vec<&DealRow> = deals.iter().filter(|r| r.dir == dir && r.bought).collect();
        let max = bd.iter().map(|x| concurrent(deals, x)).max().unwrap_or(0);
        let r4 = &t4[dir];
        near(num(&r4[1]).unwrap(), max as f64, "максимум одновременно");
        let durs: Vec<f64> = (bd.iter())
            .filter_map(|r| {
                r.end
                    .map(|e| ((e - r.start).num_milliseconds() as f64 / 1000.0).round())
            })
            .collect();
        if durs.is_empty() {
            assert_eq!(r4[2], "", "нет длительностей — пусто");
        } else {
            let avg = (durs.iter().sum::<f64>() / durs.len() as f64).round();
            near(num(&r4[2]).unwrap(), avg, "средняя длительность");
        }
        for (k, cap) in LADDER.iter().enumerate() {
            let want = cap.parse::<f64>().unwrap() * max as f64;
            near(num(&r4[3 + k]).unwrap(), want, "баланс");
        }
    }
}

#[tokio::test]
async fn capital_workbook_from_recorded_data() {
    let all = deals();
    let m = missed_data().await;
    let now = Utc.with_ymd_and_hms(2026, 11, 5, 12, 20, 0).unwrap();
    let data = capital_data(&all, &m, None, None, "за всё время", now);
    assert_eq!(data.deals.len(), 400);
    assert!(
        data.deals.windows(2).all(|w| w[0].start <= w[1].start),
        "старые сверху"
    );
    // Эпизоды: 02.11 BingX → DEX — 1 со сделкой; DEX → BingX — 2 (запись сделки и кандидат с тем же началом — один
    // эпизод; несравнимая запись сделки не входит); 03.11 DEX → BingX — 1.
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
    // Эпизод со сделкой: «что ограничило» — у пиковой записи (наибольшее упущено).
    let lim: u64 = data.days.iter().map(|r| r.limits.iter().sum::<u64>()).sum();
    assert_eq!(lim, 4, "по одному «что ограничило» на эпизод");

    let bytes = workbook(&data, None).expect("книга");
    if let Ok(out) = std::env::var("REPORT_CAPITAL_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
    let x = unzip(&bytes);
    let wb = &x["xl/workbook.xml"];
    for s in ["Сделки", "Упущено из-за потолка", "Итоги"] {
        assert!(wb.contains(&format!("name=\"{s}\"")), "лист {s}");
    }
    let strings = &x["xl/sharedStrings.xml"];
    for h in [
        "№ сделки",
        "Монета",
        "Контракт",
        "Биржа покупки",
        "Биржа продажи",
        "Направление",
        "Покупка состоялась",
        "Статус",
        "Длительность, с",
        "Вложено, USDT",
        "Получено, USDT",
        "Прибыль по плану, USDT",
        "Итог по факту, USDT",
        "Возврат USDT (состояние)",
        "День (UTC)",
        "Эпизодов",
        "Эпизодов со сделкой",
        "Прибыль на нашем бюджете (модель)",
        "Прибыль при лимите 100 USDT (модель)",
        "Сколько раз ограничил: выгодный объём",
        "Сколько раз ограничил: другое",
    ] {
        assert!(strings.contains(h), "заголовок «{h}»");
    }
    let (s1, s3) = (
        &x["xl/worksheets/sheet1.xml"],
        &x["xl/worksheets/sheet3.xml"],
    );
    assert!(
        s1.contains("<f>COUNTIFS('Сделки'!$F$2:$F$401,F2,"),
        "одновременно открыто — COUNTIFS"
    );
    assert!(
        s1.contains("<f>ROUND((J2-I2)*86400,0)</f>"),
        "длительность — формулой"
    );
    for f in [
        "COUNTIFS(",
        "SUMIFS(",
        "_xlfn.MAXIFS(",
        "AVERAGEIFS(",
        "'Упущено из-за потолка'!$C$2:$C$4",
    ] {
        assert!(s3.contains(f), "«Итоги»: {f}");
    }
    for f in ["XLOOKUP", "FILTER(", "UNIQUE("] {
        assert!(!s1.contains(f) && !s3.contains(f), "без {f}");
    }

    match recalc(&bytes) {
        Some(lo) => {
            let sd = &lo["Сделки"];
            assert_eq!(sd.len(), 401);
            for (i, r) in data.deals.iter().enumerate() {
                let want = concurrent(&data.deals, r) as f64;
                near(num(&sd[i + 1][16]).unwrap(), want, "одновременно открыто");
            }
            check_summary(&lo, &data.deals, &data.days);
        }
        None => eprintln!("LibreOffice нет — пересчёт формул пропущен, проверены строки формул"),
    }

    // Деление по дням: предел меньше книги — несколько частей, каждая не больше предела, дни подряд.
    // Книга пишет время создания (docProps) — размер плавает на байты; 9/10 — заведомо меньше целой книги.
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
    assert_eq!(
        got[0].name,
        format!("arb_capital_2026-11-05_part1of{n}.xlsx")
    );
    assert!(got[0].caption.contains("часть 1 из"));
    // Слишком маленький предел — понятная ошибка, а не потеря данных.
    let err = files(&data, 1000, "p", "c", &mut |_| true).unwrap_err();
    assert!(err.contains("больше предела"), "{err}");
}

#[test]
fn custom_interval_parse() {
    let p = parse_range(" 01.11.26 – 07.11.26 ").unwrap();
    let day = |m, dd| NaiveDate::from_ymd_opt(2026, m, dd).unwrap();
    assert_eq!(p, Period::Range(day(11, 1), day(11, 7)));
    let (a, b) = p.bounds(Utc::now());
    assert_eq!(
        a.unwrap(),
        Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap()
    );
    assert_eq!(
        b.unwrap(),
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
    let now = Utc.with_ymd_and_hms(2026, 11, 11, 15, 30, 0).unwrap();
    assert_eq!(
        Period::Today.bounds(now).0.unwrap(),
        Utc.with_ymd_and_hms(2026, 11, 11, 0, 0, 0).unwrap()
    );
    assert_eq!(
        Period::Days(3).bounds(now).0.unwrap(),
        now - Duration::days(3)
    );
    assert_eq!(Period::All.bounds(now), (None, None));
}
