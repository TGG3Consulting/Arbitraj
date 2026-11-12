//! Файл: core/daily_rows.rs
//! Что делает: решение 257 — итог дня бота по записям таблицы daily_result (строка на день UTC, её пишет
//! daily_commit.rs при каждом пересчёте итога дня): для отчёта «Итоги сделок», лист «По дням» — колонка «Итог дня бота
//! (запись бота)» рядом с итогом дня, собранным из начислений сделок (Deal.day_book). Итог дня = факт дня (final_usd)
//! плюс оценки позиций, учтённые в этот день (prov_on — как DayView.result_usd). Только чтение.
//! Кто вызывает: report/results.rs (results_file).
//! От чего зависит: daily.rs (Book, prov_on), storage/db.rs.
//! Настройки: нет.

use chrono::NaiveDate;
use rust_decimal::Decimal;

use super::daily::{Book, prov_on};
use super::engine::Core;

/// Итог одного дня бота (запись daily_result).
#[derive(Debug, Clone, PartialEq)]
pub struct BotDay {
    pub date: NaiveDate,
    /// Факт дня (DayView.final_usd).
    pub final_usd: Decimal,
    /// Оценка позиций, учтённых в этот день (DayView.provisional_usd).
    pub provisional_usd: Decimal,
}

impl Core {
    /// Что делает: итоги дней бота из daily_result (все дни, по возрастанию даты). Запись, которую не разобрать, —
    /// пропускается с причиной в списке ошибок. Возвращает: (дни, ошибки разбора). Ошибки: текст — база не прочитана.
    pub async fn bot_days(&self) -> Result<(Vec<BotDay>, Vec<String>), String> {
        let rows = self
            .inner
            .deps
            .db
            .call(|c| {
                let mut st = c.prepare("SELECT day, body FROM daily_result ORDER BY day")?;
                let it =
                    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                it.collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|e| format!("итог дня бота (daily_result) не прочитан: {e}"))?;
        let (mut out, mut bad) = (Vec::new(), Vec::new());
        for (day, body) in rows {
            match (
                day.parse::<NaiveDate>(),
                serde_json::from_str::<Book>(&body),
            ) {
                (Ok(date), Ok(b)) => out.push(BotDay {
                    date,
                    final_usd: b.final_usd,
                    provisional_usd: prov_on(&b, date),
                }),
                _ => bad.push(format!("запись итога дня {day} не разобрана")),
            }
        }
        Ok((out, bad))
    }
}
