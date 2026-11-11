//! Файл: report/snap_sheet.rs
//! Что делает: решение 257 (Тигран 11.11 п. 10) — лист «Слепок по балансам» отчёта «Итоги сделок» из данных
//! report/snap.rs: по каждой площадке (биржа, кошелёк DEX) — строки «счёт · монета · количество · цена в USDT ·
//! стоимость = количество × цена (формула)»; нет цены — «нет цены: причина», в стоимость — текст «нет цены» (в итог не
//! входит, число таких строк — в строке итога площадки); строка «Итого <площадка>» = SUM стоимостей блока; активные
//! сделки — текстом у площадки (в итог не входят: их монеты и деньги уже в балансах площадки); внизу «Общий итог» = сумма
//! итогов площадок (формула).
//! Кто вызывает: report/results_xlsx.rs.
//! От чего зависит: rust_xlsxwriter, report/snap.rs (Place), report/results_sheet.rs.
//! Настройки: нет.

use rust_xlsxwriter::{Formula, Worksheet, XlsxError};

use super::results_sheet::{base, header, italic_note, money4};
use super::snap::Place;

/// Текст ячейки стоимости без цены (его считает COUNTIF строки итога).
const NO_PRICE: &str = "нет цены";

/// Лист «Слепок по балансам». Принимает: лист, площадки, время выгрузки текстом. Возвращает: строку (1-based) ячейки
/// общего итога в колонке F — для листа «Итог». Ошибки: XlsxError.
pub fn sheet(ws: &mut Worksheet, places: &[Place], at: &str) -> Result<u32, XlsxError> {
    ws.set_name("Слепок по балансам")?;
    let hdr = [
        "Площадка",
        "Счёт",
        "Монета",
        "Количество",
        "Цена, USDT за монету (чистыми, продажа сейчас)",
        "Стоимость, USDT",
        "Пояснение",
    ];
    header(ws, &hdr, &[24., 16., 10., 16., 18., 16., 60.])?;
    let (p, b, m4) = (base(), base().set_bold(), money4());
    let qf = base().set_num_format("#,##0.########");
    let mut row = 1u32;
    let mut totals: Vec<u32> = Vec::new();
    for pl in places {
        let first = row + 1;
        let when = pl
            .at
            .map(|t| format!("ответ площадки {} UTC", t.format("%H:%M:%S")));
        if let Some(e) = &pl.error {
            ws.write_string_with_format(row, 0, &pl.name, &p)?;
            ws.write_string_with_format(row, 6, format!("не получено: {e}"), &p)?;
            row += 1;
        }
        for l in &pl.lines {
            let i = row + 1;
            ws.write_string_with_format(row, 0, &pl.name, &p)?;
            ws.write_string_with_format(row, 1, &l.account, &p)?;
            ws.write_string_with_format(row, 2, &l.asset, &p)?;
            ws.write_number_with_format(row, 3, l.qty, &qf)?;
            match &l.price {
                Ok(x) => {
                    ws.write_number_with_format(row, 4, *x, &m4)?;
                    ws.write_formula_with_format(row, 5, Formula::new(format!("=D{i}*E{i}")), &m4)?;
                    ws.write_string_with_format(row, 6, when.clone().unwrap_or_default(), &p)?;
                }
                Err(why) => {
                    ws.write_string_with_format(row, 4, format!("{NO_PRICE}: {why}"), &p)?;
                    ws.write_string_with_format(row, 5, NO_PRICE, &p)?;
                }
            }
            row += 1;
        }
        for d in &pl.deals {
            ws.write_string_with_format(row, 0, &pl.name, &p)?;
            ws.write_string_with_format(row, 6, d, &p)?;
            row += 1;
        }
        let last = row;
        ws.write_string_with_format(row, 0, format!("Итого {}", pl.name), &b)?;
        let (sum, cnt) = if last >= first {
            (
                format!("=SUM(F{first}:F{last})"),
                format!(
                    "=IF(COUNTIF(F{first}:F{last},\"{NO_PRICE}\")=0,\"\",\"без цены строк: \"&COUNTIF(F{first}:F{last},\"{NO_PRICE}\")&\" — в итог не вошли\")"
                ),
            )
        } else {
            ("=0".to_string(), "=\"\"".to_string())
        };
        ws.write_formula_with_format(row, 5, Formula::new(sum), &m4.clone().set_bold())?;
        ws.write_formula_with_format(row, 6, Formula::new(cnt), &p)?;
        totals.push(row + 1);
        row += 2;
    }
    ws.write_string_with_format(row, 0, "Общий итог, USDT", &b)?;
    let f = if totals.is_empty() {
        "=0".to_string()
    } else {
        let cells: Vec<String> = totals.iter().map(|r| format!("F{r}")).collect();
        format!("={}", cells.join("+"))
    };
    ws.write_formula_with_format(row, 5, Formula::new(f), &m4.clone().set_bold())?;
    let total = row + 1;
    let notes = [
        format!("Слепок — в момент выгрузки ({at} UTC): балансы — свежим запросом к каждой бирже (все счета, свободно + заморожено) и кошельку DEX (BNB и USDT)."),
        "Цена — как бот оценивает позицию (решение 152): сколько USDT чистыми получим, если продать всё количество сейчас на этой площадке (стакан биржи / котировка DEX кошельком, минус комиссии вне цены), делённое на количество. Котируемая монета площадки (venue.quote_asset) = 1. Нет цены — причина, в итог не входит.".into(),
        "Активные сделки — для сведения: их монеты и деньги уже в балансах площадки, в итог отдельно не прибавляются.".into(),
    ];
    for (k, t) in notes.iter().enumerate() {
        italic_note(ws, row + 2 + k as u32, t)?;
    }
    Ok(total)
}
