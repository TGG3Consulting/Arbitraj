//! Файл: tests/report_missed.rs
//! Что проверяет: решение 215 — отчёт Excel «Упущено» (report/missed_xlsx.rs) на записях «без потолка», посчитанных
//! функциями ядра на записанных данных CAKE (tests/size_common: стакан BingX ws_spot_2026-10-23, пул CAKE/USDT V2 —
//! как tests/core_uncapped.rs): итоги той же функцией, что у экрана (core/uncapped_sum.rs totals — кандидат с упущено 0
//! скрыт, сделка без итога в учёте — без факта); книга собирается, листы «Итог», «Сделки», «Кандидаты», «Топ
//! направлений» на месте, строки «Итого» — формулами SUM по столбцам денег; итоги экрана совпадают со строками листов.
//! Решение 217: старая запись сделки (посчитана после покупки) — отдельный итог deals_old и лист «Сделки — занижено».
//! «Итоги сделок» с периодом (report::results from/to) — книга собирается и на пустом периоде.
//! Записывает книгу в REPORT_MISSED_OUT (если задан) для просмотра.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{size_uncapped, uncapped_types, uncapped_sum, uncapped_report}, arb::report, flate2,
//! tests/size_common.

mod size_common;

use std::collections::HashMap;
use std::io::Read;

use arb::core::size_uncapped::{At, uncapped};
use arb::core::types::{Asset, Direction, VenueId};
use arb::core::uncapped_report::MissedData;
use arb::core::uncapped_sum::totals;
use arb::core::uncapped_types::{
    BASIS_DECISION, BASIS_LIVE_AFTER_BUY, Battle, Brief, Episode, missed,
};
use chrono::{Duration, TimeZone, Utc};
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{CAKE, Rec, d, params};

/// Направление CAKE на площадках теста.
fn dir(d: Dir) -> Direction {
    let (b, s) = match d {
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

/// Запись «без потолка» по расчёту ядра на записанных данных. Принимает: данные, направление, бюджет, вердикт, id
/// сделки, минуты от начала теста, порог прибыли «проходит». Ошибок нет (паника — расчёт не удался).
async fn brief(
    rec: &Rec,
    dr: Dir,
    budget: &str,
    verdict: &str,
    deal: Option<&str>,
    min: i64,
    floor: Decimal,
) -> Brief {
    let mut c = RecCurve {
        rec,
        dir: dr,
        slip: Decimal::ONE,
    };
    let ladder = [d("20"), d("100"), d("1000")];
    let p = params(rec, dr, d("1000000"));
    let u = uncapped(
        &mut c,
        &p,
        d(budget),
        &ladder,
        |pt| pt.profit >= floor,
        None,
    )
    .await
    .unwrap();
    let at = Utc.with_ymd_and_hms(2026, 11, 7, 10, 0, 0).unwrap() + Duration::minutes(min);
    let ab = u.at_budget.at.unwrap_or(At {
        qty: Decimal::ZERO,
        invested: Decimal::ZERO,
        profit: Decimal::ZERO,
    });
    let opt = At::of(&u.opt);
    Brief {
        at,
        direction: dir(dr),
        battle: Battle {
            verdict: verdict.into(),
            reason: (verdict == "reject").then(|| "прибыль ниже порога".into()),
            deal_id: deal.map(String::from),
            dry: false,
            qty: ab.qty,
            invested: ab.invested,
            profit: ab.profit,
            budget: Some(d(budget)),
            plan_budget: None,
            free: None,
            parts: None,
            window_from: deal.map(|_| at - Duration::seconds(42)),
        },
        limited_by: "per_trade".into(),
        missed: missed(verdict, u.opt_passes, opt.profit, &u.at_budget),
        opt,
        opt_passes: u.opt_passes,
        min_plus: u.min_plus.as_ref().map(At::of),
        at_budget: u.at_budget.clone(),
        levels: u.levels.clone(),
        ep: Episode {
            first_at: at,
            last_at: at + Duration::seconds(90),
            count: 7,
            capped_count: 5,
            peak_at: at + Duration::seconds(30),
        },
        budget: d(budget),
        // Решение 236: сделки теста посчитаны по данным момента входа.
        basis: BASIS_DECISION.into(),
        decided_at: deal.map(|_| at),
    }
}

/// Файлы xlsx (zip) по имени: центральный каталог, deflate или без сжатия. Ошибок нет (паника — не zip).
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
        let (nl, el, cl, off) = (u16le(p + 28), u16le(p + 30), u16le(p + 32), u32le(p + 42));
        let name = String::from_utf8(b[p + 46..p + 46 + nl].to_vec()).unwrap();
        let data = off + 30 + u16le(off + 26) + u16le(off + 28);
        let raw = &b[data..data + csize];
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

#[tokio::test]
async fn missed_workbook_from_recorded_calc() {
    let rec = Rec::load("report-missed").await;
    let zero = Decimal::ZERO;
    let briefs = vec![
        // Новые сверху — как выборка базы.
        brief(
            &rec,
            Dir::BingxToDex,
            "1000000",
            "accept",
            None,
            40,
            d("1000000000"),
        )
        .await,
        brief(&rec, Dir::DexToBingx, "20", "reject", None, 30, zero).await,
        brief(&rec, Dir::BingxToDex, "20", "deal", Some("d2"), 20, zero).await,
        brief(&rec, Dir::BingxToDex, "20", "deal", Some("d1"), 10, zero).await,
        // Решение 217: старая запись сделки (посчитана после покупки) — отдельный итог и лист.
        {
            let mut b = brief(&rec, Dir::BingxToDex, "20", "deal", Some("d0"), 5, zero).await;
            b.basis = BASIS_LIVE_AFTER_BUY.into();
            b.decided_at = None;
            b
        },
    ];
    assert_eq!(
        briefs[0].missed, zero,
        "кандидат без упущенного — для проверки скрытия"
    );
    let t = totals(briefs, &[d("20"), d("100"), d("1000")], |id| {
        (id == "d1").then(|| d("0.5"))
    });
    assert_eq!(t.hidden, 1);
    assert_eq!((t.deals.count, t.cands.count), (2, 1));
    assert_eq!(
        t.deals_old.count, 1,
        "старая запись сделки — не в итоге сделок"
    );
    assert_eq!((t.deals.fact, t.deals.fact_count), (d("0.5"), 1));
    assert_eq!(t.top.len(), 2);
    assert!(t.top[0].missed >= t.top[1].missed);
    let data = MissedData {
        totals: t,
        ladder_error: None,
        off: None,
        labels: HashMap::from([(dir(Dir::BingxToDex).asset, "CAKE / Cake".to_string())]),
    };
    let deals = arb::report::missed_rows::deals(&data, false);
    assert_eq!(deals.len(), 2);
    let old = arb::report::missed_rows::deals(&data, true);
    assert_eq!(old.len(), 1);
    assert_eq!(old[0].basis, "посчитано после покупки — занижено");
    assert_eq!(
        deals[0].basis,
        "все данные на момент входа: стаканы, пулы, σ, время в пути, курсы, газ"
    );
    // Решение 236: расчёт на вложенных — точка «на нашем бюджете» записи.
    let with_point = (data.totals.items.iter())
        .filter(|i| i.deal && !i.b.understated() && i.b.at_budget.at.is_some())
        .count();
    assert_eq!(
        deals.iter().filter(|r| r.at_invested.is_some()).count(),
        with_point
    );
    assert!(deals[0].time < deals[1].time, "старые сверху");
    assert_eq!((deals[0].fact, deals[1].fact), (Some(0.5), None));
    assert_eq!(deals[0].coin, "CAKE / Cake");
    assert_eq!(deals[0].dir, "BingX → DEX");
    assert!(deals[0].window.ends_with("(42 с)"), "{}", deals[0].window);
    let sum_missed: f64 = deals.iter().map(|r| r.missed).sum();
    let screen = arb::report::missed_rows::num(data.totals.deals.missed, 4);
    assert!(
        (sum_missed - screen).abs() < 1e-3,
        "итог экрана = строки листа"
    );
    let cands = arb::report::missed_rows::candidates(&data);
    assert_eq!(cands.len(), 1);
    assert!(cands[0].verdict.starts_with("отказ: "));

    let now = Utc.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap();
    let bytes =
        arb::report::missed(&data, Some(now - Duration::days(1)), None, now).expect("книга");
    if let Ok(out) = std::env::var("REPORT_MISSED_OUT") {
        std::fs::write(out, &bytes).unwrap();
    }
    let files = unzip(&bytes);
    let wb = &files["xl/workbook.xml"];
    for name in [
        "Итог",
        "Сделки",
        "Кандидаты",
        "Топ направлений",
        "Сделки — занижено",
    ] {
        assert!(wb.contains(&format!("name=\"{name}\"")), "лист {name}");
    }
    // Сделки — занижено (лист 5): 1 строка → «Итого» в строке 3.
    let s5 = &files["xl/worksheets/sheet5.xml"];
    assert!(s5.contains("<f>SUM(K2:K2)</f>"), "Сделки — занижено");
    // Сделки: 2 строки → «Итого» в строке 4, SUM по деньгам D, F, G, H (расчёт на вложенных, решение 236), I, J, K.
    let s2 = &files["xl/worksheets/sheet2.xml"];
    for c in ['D', 'F', 'G', 'H', 'I', 'J', 'K'] {
        assert!(s2.contains(&format!("<f>SUM({c}2:{c}3)</f>")), "Сделки {c}");
    }
    // Кандидаты: 1 строка → SUM по F, G, I, J, L.
    let s3 = &files["xl/worksheets/sheet3.xml"];
    for c in ['F', 'G', 'I', 'J', 'L'] {
        assert!(
            s3.contains(&format!("<f>SUM({c}2:{c}2)</f>")),
            "Кандидаты {c}"
        );
    }
    let shared = &files["xl/sharedStrings.xml"];
    for t in [
        "Итого",
        "без потолка",
        "Исполненные сделки",
        "Не исполненные (кандидаты)",
        "лимит на сделку",
        "Сделки: посчитано после покупки — занижено",
        "Упущено против вложенного, USDT",
        "Потолок (макс. прибыль без лимита): вложить, USDT",
    ] {
        assert!(shared.contains(t), "строка «{t}»");
    }
}

#[test]
fn results_with_period_builds() {
    let now = Utc.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap();
    let from = Some(now - Duration::days(1));
    assert!(arb::report::in_period(now, from, None));
    assert!(!arb::report::in_period(now - Duration::days(2), from, None));
    assert!(!arb::report::in_period(now, None, Some(now)));
    let bytes = arb::report::results(&[], from, None, now).expect("книга без сделок");
    assert!(unzip(&bytes)["xl/workbook.xml"].contains("name=\"Сделки\""));
    // «Упущено» за период без записей — книга с пустыми листами и нулевыми итогами.
    let empty = MissedData {
        totals: Default::default(),
        ladder_error: Some("uncapped.ladder_usdt не задана".into()),
        off: None,
        labels: HashMap::new(),
    };
    let bytes = arb::report::missed(&empty, from, None, now).expect("пустая книга «Упущено»");
    assert!(unzip(&bytes)["xl/worksheets/sheet2.xml"].contains("<f>SUM(D2:D1)</f>"));
}
