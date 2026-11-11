//! Файл: report/cap_capital.rs
//! Что делает: решение 256 — таблицы 3 и 4 листа «Итоги» отчёта «Сделки, лимиты и капитал» (только формулы):
//! 3. «Сколько добавил бы больший лимит»: SUMIFS прибыли модели на нашем бюджете и на каждом потолке по листу
//!    «Упущено из-за потолка»; прибавка между соседними потолками — разность ячеек.
//! 4. «Сколько держать на балансе»: _xlfn.MAXIFS столбца «Одновременно открыто в момент начала» листа «Сделки» (сделки с
//!    покупкой), AVERAGEIFS длительности; рекомендованный баланс = лимит (строка над таблицей, uncapped.ladder_usdt) ×
//!    максимум одновременных.
//! Кто вызывает: report/cap_summary.rs (summary).
//! От чего зависит: rust_xlsxwriter, cap_summary.rs (W, Ctx, total, dr, mr), cap_sheets.rs (YES, cap_text, col, d).
//! Настройки: нет (потолки — uncapped.ladder_usdt, приходят готовым списком).

use rust_decimal::prelude::ToPrimitive;
use rust_xlsxwriter::XlsxError;

use super::cap_sheets::{YES, cap_text, col, d};
use super::cap_summary::{Ctx, W, dr, mr, total};

/// Таблица 3 «Сколько добавил бы больший лимит». Ошибки: XlsxError.
pub(super) fn ladder(w: &mut W, cx: &Ctx) -> Result<(), XlsxError> {
    let k = cx.ladder.len();
    let mut hdr = vec![
        "Направление".to_string(),
        "Прибыль на нашем бюджете (модель)".to_string(),
    ];
    for c in cx.ladder {
        hdr.push(format!("Прибыль при лимите {} USDT (модель)", cap_text(*c)));
    }
    for p in cx.ladder.windows(2) {
        hdr.push(format!(
            "Прибавка {} → {} USDT (модель)",
            cap_text(p[0]),
            cap_text(p[1])
        ));
    }
    w.table(
        "3. Сколько добавил бы больший лимит (модель, эпизоды)",
        &hdr,
    )?;
    let first = w.row + 1;
    for dir in cx.dirs {
        let n = w.row + 1;
        w.ws.write_string_with_format(w.row, 0, dir, &w.int)?;
        let m = w.money.clone();
        // «Упущено»: E — на нашем бюджете, F… — потолки.
        for i in 0..=k as u16 {
            let f = format!("=SUMIFS({},{},$A{n})", mr(cx, 4 + i), mr(cx, 1));
            w.f(1 + i, f, &m)?;
        }
        for i in 1..k as u16 {
            let (a, b) = (col(1 + i), col(2 + i));
            w.f(2 + k as u16 + i - 1, format!("={b}{n}-{a}{n}"), &m)?;
        }
        w.row += 1;
    }
    // Столбцы: 1 — наш бюджет, 2..=1+k — потолки, дальше k − 1 прибавок: всего до 2k (без потолков — один).
    let cols: Vec<(u16, bool)> = (1..=(2 * k as u16).max(1)).map(|c| (c, true)).collect();
    total(w, first, &cols)
}

/// Таблица 4 «Сколько держать на балансе». Ошибки: XlsxError.
pub(super) fn balance(w: &mut W, cx: &Ctx) -> Result<(), XlsxError> {
    w.row += 1;
    w.ws.write_string_with_format(w.row, 0, "4. Сколько держать на балансе", &w.bold)?;
    w.row += 1;
    // Строка лимитов (значения uncapped.ladder_usdt) — на неё ссылаются формулы баланса.
    let caps_row = w.row + 1;
    w.ws.write_string_with_format(w.row, 2, "Лимит на сделку, USDT →", &w.bold)?;
    for (i, c) in cx.ladder.iter().enumerate() {
        let v = c.to_f64().unwrap_or(0.0);
        w.ws.write_number_with_format(w.row, 3 + i as u16, v, &w.money)?;
    }
    w.row += 1;
    let mut hdr = vec![
        "Направление".to_string(),
        "Максимум одновременно открытых сделок (с покупкой)".to_string(),
        "Средняя длительность сделки с покупкой, с".to_string(),
    ];
    for c in cx.ladder {
        hdr.push(format!(
            "Баланс при лимите {} USDT = лимит × максимум, USDT",
            cap_text(*c)
        ));
    }
    for (c, h) in hdr.iter().enumerate() {
        w.ws.write_string_with_format(w.row, c as u16, h, &w.head)?;
    }
    w.ws.set_row_height(w.row, 45.)?;
    w.row += 1;
    let first = w.row + 1;
    let (dir, bought, conc, dur) = (
        dr(cx, d::DIR),
        dr(cx, d::BOUGHT),
        dr(cx, d::CONC),
        dr(cx, d::DUR),
    );
    for name in cx.dirs {
        let n = w.row + 1;
        w.ws.write_string_with_format(w.row, 0, name, &w.int)?;
        let cond = format!("{dir},$A{n},{bought},\"{YES}\"");
        let (int, m) = (w.int.clone(), w.money.clone());
        w.f(1, format!("=_xlfn.MAXIFS({conc},{cond})"), &int)?;
        w.f(
            2,
            format!("=IFERROR(ROUND(AVERAGEIFS({dur},{cond},{dur},\"<>\"),0),\"\")"),
            &int,
        )?;
        for i in 0..cx.ladder.len() as u16 {
            let l = col(3 + i);
            w.f(3 + i, format!("={l}${caps_row}*$B{n}"), &m)?;
        }
        w.row += 1;
    }
    let cols: Vec<(u16, bool)> = (0..cx.ladder.len() as u16).map(|i| (3 + i, true)).collect();
    total(w, first, &cols)
}
