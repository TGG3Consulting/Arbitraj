//! Файл: report/results.rs
//! Что делает: решения 193, 215, 257 — вход отчёта «Итоги сделок» (вынесено из report/mod.rs — размер файла): данные
//! книги из ядра (сделки учёта, итог дня бота daily_result, дни учёта неразобранных оценок, площадки DEX, слепок
//! балансов) и сборка книги в spawn_blocking. Одна функция results_file — для кнопки интерфейса (api/report.rs) и
//! Telegram (notify/ctl/report.rs).
//! Кто вызывает: report/mod.rs (реэкспорт), api/report.rs, notify/ctl/report.rs, tests/report_results.rs.
//! От чего зависит: core (deals, bot_days, control, daily_venue::is_dex), report/rows.rs, day_rows.rs, snap.rs,
//! results_xlsx.rs, rows_time.rs.
//! Настройки: нет.

use chrono::{DateTime, Utc};

use crate::core::Core;
use crate::core::deal::Deal;

use super::{day_rows, results_xlsx, rows, rows_time, snap};

/// Решение 257: данные «Итогов сделок», кроме сделок: итог дня бота (daily_result), дни учёта неразобранных оценок
/// (DayView.provisional), id площадок DEX (реестр шлюза), слепок балансов, что не прочитано.
#[derive(Debug, Clone, Default)]
pub struct ResultsCtx {
    pub bot: Vec<crate::core::daily_rows::BotDay>,
    pub prov_days: std::collections::BTreeMap<String, chrono::NaiveDate>,
    pub dex: std::collections::BTreeSet<String>,
    pub snap: Vec<snap::Place>,
    pub problems: Vec<String>,
}

/// Книга «Итоги сделок» (решения 193, 257) по окну [from, to) (None — без границы): сделки, закончившиеся в окне
/// (rows.rs), начисления в итог дня в окне (day_rows.rs), итог дня бота, слепок балансов.
/// Принимает: все сделки учёта, окно, остальное (ResultsCtx). Возвращает: байты .xlsx или текст ошибки.
pub fn results(deals: &[Deal], w: rows_time::Window, cx: &ResultsCtx) -> Result<Vec<u8>, String> {
    let rows = rows::rows(deals, &w, &cx.dex);
    let ledger = day_rows::day_rows(deals, &w, &cx.dex, &cx.prov_days);
    let input = results_xlsx::Input {
        rows: &rows,
        ledger: &ledger,
        bot: &cx.bot,
        snap: &cx.snap,
        window: w,
        problems: &cx.problems,
    };
    results_xlsx::workbook(&input).map_err(|e| format!("отчёт xlsx: {e}"))
}

/// Файл «Итоги сделок» за окно — одна функция для кнопки интерфейса и Telegram (решение 257): сделки учёта ядра на момент
/// вызова, итог дня бота (daily_result), площадки DEX, слепок балансов (запросы к площадкам), книга в spawn_blocking.
/// Принимает: ядро, границы, время выгрузки. Возвращает: байты .xlsx. Ошибки: текст (книга не собралась).
pub async fn results_file(
    core: &Core,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Vec<u8>, String> {
    let deals = core.deals();
    let mut cx = ResultsCtx::default();
    match core.bot_days().await {
        Ok((days, bad)) => {
            cx.bot = days;
            cx.problems.extend(bad);
        }
        Err(e) => cx.problems.push(e),
    }
    cx.prov_days = core
        .control()
        .day
        .provisional
        .into_iter()
        .map(|(id, (_, d))| (id, d))
        .collect();
    for d in &deals {
        for v in [&d.direction.buy_on.0, &d.direction.sell_on.0] {
            if crate::core::daily_venue::is_dex(core, v) {
                cx.dex.insert(v.clone());
            }
        }
    }
    cx.snap = snap::gather(core).await;
    let w = rows_time::Window { from, to, now };
    tokio::task::spawn_blocking(move || results(&deals, w, &cx))
        .await
        .map_err(|e| format!("отчёт: задача сборки прервана: {e}"))?
}
