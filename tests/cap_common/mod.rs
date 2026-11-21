//! Файл: tests/cap_common/mod.rs
//! Что делает: решение 256 — общее для тестов отчёта «Сделки, лимиты и капитал» (tests/report_capital.rs,
//! tests/report_capital_total.rs): разбор xlsx (zip) и CSV, пересчёт книги LibreOffice (если есть в среде; профиль с
//! OOXMLRecalcMode = 0 — «всегда пересчитывать при загрузке»), сравнение чисел; покупка сделки в учёте (Deal.buy) по
//! записанным вложено и количеству — как её кладёт core/fills.rs (биржа — id ордера, DEX — транзакция и маршрут).
//! Кто вызывает: tests/report_capital.rs, tests/report_capital_total.rs.
//! От чего зависит: arb::core (deal, types), flate2, LibreOffice (необязательно). Моков нет.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::Read;
use std::process::Command;
use std::sync::atomic::AtomicUsize;

use arb::core::deal::Deal;
use arb::core::types::{ChainTx, DexRoute, Side, TradeRequest, TradeResult, TradeStatus};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

/// Счётчик книг для своих папок пересчёта (тесты идут параллельно).
static BOOK: AtomicUsize = AtomicUsize::new(0);

/// Итог исполнения стороны сделки (как его возвращает адаптер): площадка, сторона, количество, средняя цена, id ордера
/// или хеш, транзакция DEX, маршрут DEX, время. Ошибок нет.
#[allow(clippy::too_many_arguments)]
pub fn trade(
    d: &Deal,
    venue: &str,
    side: Side,
    qty: Decimal,
    price: Decimal,
    venue_ref: &str,
    chain: Option<ChainTx>,
    route: Option<DexRoute>,
    at: DateTime<Utc>,
) -> TradeResult {
    TradeResult {
        request: TradeRequest {
            venue: arb::core::types::VenueId(venue.into()),
            asset: d.direction.asset.clone(),
            side,
            qty,
            worst_price: price,
            deal_id: d.id.clone(),
            route,
            exit_position: false,
            remeasure: None,
        },
        status: TradeStatus::Filled,
        filled_qty: qty,
        avg_price: price,
        fees: vec![],
        venue_ref: venue_ref.into(),
        chain,
        finished_at: at,
        executor: None,
    }
}

/// Покупка на бирже по вложенному (1 монета по цене «вложено»), id ордера — «<id сделки>b». Ошибок нет.
pub fn bought(d: &mut Deal) {
    let b = d.direction.buy_on.0.clone();
    let t = trade(
        d,
        &b,
        Side::Buy,
        Decimal::ONE,
        d.invested,
        &format!("{}b", d.id),
        None,
        None,
        d.started_at,
    );
    d.buy = Some(t);
}

/// Файлы xlsx (zip) по имени: центральный каталог, deflate или без сжатия (как tests/report_missed.rs).
pub fn unzip(b: &[u8]) -> HashMap<String, String> {
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
pub fn csv(text: &str) -> Vec<Vec<String>> {
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
pub fn recalc(bytes: &[u8], sheets: &[&str]) -> Option<HashMap<String, Vec<Vec<String>>>> {
    Command::new("soffice").arg("--version").output().ok()?;
    let n = BOOK.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("arb-cap-lo-{}-{n}", std::process::id()));
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
    for s in sheets {
        let t = std::fs::read_to_string(dir.join(format!("cap-{s}.csv"))).unwrap();
        out.insert(s.to_string(), csv(&t));
    }
    let _ = std::fs::remove_dir_all(&dir);
    Some(out)
}

/// Число из ячейки CSV (пусто — None; «12.5%» — 0.125: процентный формат LibreOffice выгружает со знаком). Ошибок нет.
pub fn num(s: &str) -> Option<f64> {
    let t = s.trim();
    match t.strip_suffix('%') {
        Some(p) => p.parse::<f64>().ok().map(|v| v / 100.0),
        None => t.parse().ok(),
    }
}

/// Сравнение чисел с допуском. Паника — не совпало.
pub fn near(a: f64, b: f64, what: &str) {
    assert!((a - b).abs() < 1e-6 * (1.0 + b.abs()), "{what}: {a} ≠ {b}");
}
