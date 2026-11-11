//! Файл: report/snap_sheet.rs
//! Что делает: решение 257 (Тигран 11.11 п. 10; доработка по приёмке t90, п. 1) — лист «Слепок по балансам» из
//! report/snap.rs: по каждой площадке строки «счёт · монета · количество · цена в USDT · стоимость = количество × цена
//! (формула) · пояснение»; нет данных / нет цены — текстом с причиной (в стоимость — «нет данных» / «нет цены», в итог не
//! входит). «Итого <площадка>»: площадка не ответила — «нет данных: причина» (не 0); иначе SUM блока и пометка, если
//! неполно (счёт не прочитан, строки без цены / без данных). Сделки и «не учитывается» — строками у площадки. «Общий
//! итог» = SUM итогов площадок (текст «нет данных» не складывается) и пометка: полный или неполный (сколько площадок не
//! ответило, сколько строк вне итога). Слепок не снят (нет настройки) — одна строка с причиной, итог «нет данных».
//! Кто вызывает: report/results_xlsx.rs.
//! От чего зависит: rust_xlsxwriter, report/snap.rs, report/results_sheet.rs.
//! Настройки: нет.

use rust_xlsxwriter::{Formula, Worksheet, XlsxError};

use super::results_sheet::{base, header, italic_note, money4};
use super::snap::{Place, Snapshot};

/// Текст стоимости без цены и без данных.
const NO_PRICE: &str = "нет цены";
const NO_DATA: &str = "нет данных";

/// Блок площадки. Возвращает: (следующая строка 0-based, строка итога 1-based, не ответила, строк вне итога).
fn block(
    ws: &mut Worksheet,
    pl: &Place,
    mut row: u32,
) -> Result<(u32, u32, bool, usize), XlsxError> {
    let (p, b, m4) = (base(), base().set_bold(), money4());
    let qf = base().set_num_format("#,##0.########");
    let first = row + 1;
    let mut out = 0usize;
    for l in &pl.lines {
        let i = row + 1;
        for (c, t) in [(0u16, &pl.name), (1, &l.account), (2, &l.asset)] {
            ws.write_string_with_format(row, c, t, &p)?;
        }
        match (&l.qty, &l.price) {
            (Err(e), _) => {
                ws.write_string_with_format(row, 3, format!("{NO_DATA}: {e}"), &p)?;
                ws.write_string_with_format(row, 5, NO_DATA, &p)?;
                out += 1;
            }
            (Ok(x), Err(e)) => {
                ws.write_number_with_format(row, 3, *x, &qf)?;
                ws.write_string_with_format(row, 4, format!("{NO_PRICE}: {e}"), &p)?;
                ws.write_string_with_format(row, 5, NO_PRICE, &p)?;
                out += 1;
            }
            (Ok(x), Ok(pr)) => {
                ws.write_number_with_format(row, 3, *x, &qf)?;
                ws.write_number_with_format(row, 4, *pr, &m4)?;
                ws.write_formula_with_format(row, 5, Formula::new(format!("=D{i}*E{i}")), &m4)?;
            }
        }
        ws.write_string_with_format(row, 6, &l.note, &p)?;
        row += 1;
    }
    for d in &pl.deals {
        ws.write_string_with_format(row, 0, &pl.name, &p)?;
        ws.write_string_with_format(row, 6, d, &p)?;
        row += 1;
    }
    for u in &pl.unknown {
        ws.write_string_with_format(row, 0, &pl.name, &p)?;
        ws.write_string_with_format(row, 6, format!("не учитывается: {u}"), &p)?;
        row += 1;
    }
    let last = row;
    ws.write_string_with_format(row, 0, format!("Итого {}", pl.name), &b)?;
    if let Some(e) = &pl.error {
        ws.write_string_with_format(row, 5, format!("{NO_DATA}: {e}"), &b)?;
        ws.write_string_with_format(row, 6, "площадка не ответила — в общий итог не вошла", &p)?;
        return Ok((row + 2, row + 1, true, out));
    }
    let sum = if last >= first {
        format!("=SUM(F{first}:F{last})")
    } else {
        "=0".into()
    };
    ws.write_formula_with_format(row, 5, Formula::new(sum), &m4.clone().set_bold())?;
    let mut why: Vec<String> = pl.partial.clone();
    if out > 0 {
        why.push(format!(
            "строк без цены или без данных: {out} — в итог не вошли"
        ));
    }
    let note = if why.is_empty() {
        "полно".to_string()
    } else {
        format!("неполно: {}", why.join("; "))
    };
    ws.write_string_with_format(row, 6, note, &p)?;
    Ok((row + 2, row + 1, false, out))
}

/// Лист «Слепок по балансам». Принимает: лист, слепок, время выгрузки текстом. Возвращает: строку (1-based) общего
/// итога (колонка F — сумма, G — полнота) — для листа «Итог». Ошибки: XlsxError.
pub fn sheet(ws: &mut Worksheet, s: &Snapshot, at: &str) -> Result<u32, XlsxError> {
    ws.set_name("Слепок по балансам")?;
    let hdr = [
        "Площадка",
        "Счёт",
        "Монета",
        "Количество",
        "Цена, USDT за монету",
        "Стоимость, USDT",
        "Пояснение",
    ];
    header(ws, &hdr, &[26., 16., 12., 16., 18., 18., 70.])?;
    let (p, b, m4) = (base(), base().set_bold(), money4());
    let mut row = 1u32;
    let (mut totals, mut silent, mut outside) = (Vec::new(), 0usize, 0usize);
    if let Some(e) = &s.problem {
        ws.write_string_with_format(row, 6, e, &p)?;
        row += 2;
    }
    for pl in &s.places {
        let (next, t, no, out) = block(ws, pl, row)?;
        totals.push(t);
        silent += no as usize;
        outside += out;
        row = next;
    }
    ws.write_string_with_format(row, 0, "Общий итог, USDT", &b)?;
    let cells: Vec<String> = totals.iter().map(|r| format!("F{r}")).collect();
    let note = if let Some(e) = &s.problem {
        ws.write_string_with_format(row, 5, format!("{NO_DATA}: {e}"), &b)?;
        format!("неполный: {e}")
    } else {
        let f = if cells.is_empty() {
            "=0".to_string()
        } else {
            format!("=SUM({})", cells.join(","))
        };
        ws.write_formula_with_format(row, 5, Formula::new(f), &m4.clone().set_bold())?;
        let mut why = Vec::new();
        if silent > 0 {
            why.push(format!(
                "не ответили площадок: {silent} из {}",
                s.places.len()
            ));
        }
        if outside > 0 {
            why.push(format!("строк без цены или без данных: {outside}"));
        }
        if why.is_empty() {
            "полный".into()
        } else {
            format!("неполный: {}", why.join("; "))
        }
    };
    ws.write_string_with_format(row, 6, note, &b)?;
    let total = row + 1;
    let notes = [
        format!("Слепок — в момент выгрузки ({at} UTC), общий срок — настройка report.snapshot_timeout_ms. Биржи — все счета (свободно + заморожено), чтением адаптера; кошельки DEX — BNB, USDT и монеты своих сделок (прочие токены бот не читает)."),
        "Цена — котировка площадки, где монета лежит (Venue::quote: стакан биржи в памяти адаптера / пулы DEX), продажа всего количества, чистыми от комиссии продажи в котируемой монете или самой монете, / количество. Только чтение: путь ордера (кэш REST-снимков, память неудач) не используется — торговлю отчёт не меняет. Котируемая монета площадки = 1.".into(),
        "Сделки — строкой у площадки, где сейчас их монеты или деньги по шагу сделки: на бирже они уже в её балансах (в итог отдельно не прибавляются); на кошельке DEX — монета сделки строкой кошелька; на шаге перевода — в блоке «В пути» (пока вывод не ушёл с площадки, монеты есть и в её балансе — возможен двойной счёт, он подписан в строке).".into(),
    ];
    for (k, t) in notes.iter().enumerate() {
        italic_note(ws, row + 2 + k as u32, t)?;
    }
    Ok(total)
}
