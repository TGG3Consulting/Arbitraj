//! Файл: report/results.rs
//! Что делает: решения 193, 215, 257 — вход отчёта «Итоги сделок» (вынесено из report/mod.rs — размер файла): данные
//! книги из ядра (сделки учёта, итог дня бота daily_result, дни учёта неразобранных оценок, площадки DEX, слепок
//! балансов) и сборка книги в spawn_blocking. Одна функция results_file — для кнопки интерфейса (api/report.rs) и
//! Telegram (notify/ctl/report.rs).
//! Кто вызывает: report/mod.rs (реэкспорт), api/report.rs, notify/ctl/report.rs, tests/report_results.rs.
//! От чего зависит: core (deals, bot_days, control, daily_venue::is_dex), report/rows.rs, day_rows.rs, snap.rs,
//! results_xlsx.rs, rows_time.rs.
//! Настройки: report.long_deal_s (порог строки «Сделок дольше …»); report.snapshot_timeout_ms — у snap.rs.

use chrono::{DateTime, Utc};

use crate::core::Core;
use crate::core::deal::Deal;

use super::{day_rows, results_xlsx, rows, rows_time, snap};

/// Решение 257: данные «Итогов сделок», кроме сделок: итог дня бота (daily_result), дни учёта неразобранных оценок
/// (DayView.provisional), id площадок DEX (реестр шлюза), слепок балансов, что не прочитано.
/// long_deal_s — порог «Сделок дольше …» (report.long_deal_s; Err — причина, почему его нет).
#[derive(Debug, Clone)]
pub struct ResultsCtx {
    pub bot: Vec<crate::core::daily_rows::BotDay>,
    pub prov_days: std::collections::BTreeMap<String, chrono::NaiveDate>,
    pub dex: std::collections::BTreeSet<String>,
    pub snap: snap::Snapshot,
    pub problems: Vec<String>,
    pub long_deal_s: Result<u64, String>,
}

/// Настройка порога «Сделок дольше …».
pub const LONG_DEAL: &str = "report.long_deal_s";

impl Default for ResultsCtx {
    /// Пустые данные; порога нет — «не задана настройка report.long_deal_s» (как у пустого хранилища настроек).
    fn default() -> Self {
        ResultsCtx {
            bot: vec![],
            prov_days: Default::default(),
            dex: Default::default(),
            snap: Default::default(),
            problems: vec![],
            long_deal_s: Err(format!("не задана настройка {LONG_DEAL}")),
        }
    }
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
        long_deal_s: &cx.long_deal_s,
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
    results_with_snapshot(core, rows_time::Window { from, to, now }, None).await
}

pub(crate) async fn results_with_snapshot(
    core: &Core,
    w: rows_time::Window,
    source: Option<(&super::archive::Archive, snap::Snapshot)>,
) -> Result<Vec<u8>, String> {
    let deals = match &source {
        Some((a, _)) => a.deals.clone(),
        None => std::sync::Arc::new(core.deals()),
    };
    let mut cx = ResultsCtx::default();
    let days = match &source {
        Some((a, _)) => {
            crate::core::daily_rows::bot_days_from(
                &a.db,
                w.from.map(|t| t.date_naive()),
                w.to.map(|t| (t - chrono::Duration::nanoseconds(1)).date_naive()),
            )
            .await
        }
        None => core.bot_days().await,
    };
    match days {
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
    cx.dex = crate::core::chain::dexes(core)
        .into_iter()
        .map(|v| v.0)
        .collect();
    cx.long_deal_s = core
        .inner
        .deps
        .settings
        .integer(LONG_DEAL, crate::settings::read::At::default())
        .map_err(|e| e.message);
    cx.snap = match source {
        Some((_, snapshot)) => snapshot,
        None => snap::gather(core).await,
    };
    tokio::task::spawn_blocking(move || results(&deals, w, &cx))
        .await
        .map_err(|e| format!("отчёт: задача сборки прервана: {e}"))?
}
