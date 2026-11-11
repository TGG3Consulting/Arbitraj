//! Файл: report/mod.rs
//! Что делает: отчёты для скачивания из интерфейса и Telegram. Решение 193 — «Итоги сделок» (arb_results_<дата>.xlsx)
//! на сделках из учёта ядра на момент скачивания. Решение 215 — период у «Итогов сделок» (from/to; без них — за всё
//! время; решение 257 — по времени конца сделки) и отчёт «Упущено» (arb_missed_<дата>.xlsx)
//! по данным экрана «Упущено из-за потолка»; файлы отчётов одной функцией на отчёт — для api/report.rs и кнопки
//! «📊 Отчёты» чата «Управление» (notify/ctl/report.rs).
//! Решение 257 — «Итоги сделок» по окну UTC: сделки, закончившиеся в окне (и остановленные, «Беру на себя» — оценкой),
//! итог дня как у бота (начисления сделок Deal.day_book и запись бота daily_result), «По биржам» по правилу владельца,
//! «Слепок по балансам» (запросы к площадкам в момент выгрузки) — одна функция results_file для интерфейса и Telegram.
//! Решение 218 — отчёт пересчёта старых записей «без потолка» по окнам (recompute.rs: CSV и итоги для stdout).
//! Решение 255 — «Сигналы и сделки» (arb_signals_<дата>[_part<K>].xlsx, sig_*.rs): сигналы-окна, сделки, тики, стаканы,
//! ликвидность пулов; потоком (sig_stream.rs), несколько файлов, если больше предела отправки (sig_parts.rs).
//! Решение 256 — «Сделки, лимиты и капитал» (arb_capital_<дата>[_partKofN].xlsx, cap_*.rs): все сделки периода,
//! эпизоды «без потолка» по дням и направлениям, лист «Итоги» формулами; несколько файлов, если больше предела.
//! Кто вызывает: api/report.rs, notify/ctl/report.rs, notify/ctl/report_sig.rs (signals_send),
//! notify/ctl/report_cap.rs (cap_send.rs capital_send),
//! src/bin/uncapped_recompute.rs (recompute.rs).
//! От чего зависит: core (deals, uncapped_report, bot_days, control, daily_venue, балансы), report/rows*.rs, day_rows.rs,
//! res_*.rs, snap*.rs, results_xlsx.rs, results_sheet.rs, missed_rows.rs,
//! missed_xlsx.rs, missed_total.rs, sig_*.rs, core/signals_src.rs, cap_*.rs.
//! Настройки: нет (решение 256: потолки uncapped.ladder_usdt — через Core::uncapped_report).

pub mod cap_capital;
pub mod cap_episodes;
pub mod cap_rows;
pub mod cap_send;
pub mod cap_sheets;
pub mod cap_summary;
pub mod cap_xlsx;
pub mod day_rows;
pub mod missed_cands;
pub mod missed_labels;
pub mod missed_maxwin;
pub mod missed_rows;
pub mod missed_total;
pub mod missed_xlsx;
pub mod recompute;
pub mod res_days;
pub mod res_deals;
pub mod res_notes;
pub mod res_total;
pub mod results;
pub mod results_sheet;
pub mod results_xlsx;
pub mod rows;
pub mod rows_text;
pub mod rows_time;
pub mod sig_build;
pub mod sig_episodes;
pub mod sig_frames;
pub mod sig_link;
pub mod sig_model;
pub mod sig_notes;
pub mod sig_parts;
pub mod sig_period;
pub mod sig_pool;
pub mod sig_sheet;
pub mod sig_stream;
pub mod sig_summary;
pub mod sig_xlsx;
pub mod snap;
pub mod snap_sheet;

use chrono::{DateTime, Utc};

use crate::core::Core;
use crate::core::uncapped_report::MissedData;

pub use results::{ResultsCtx, results, results_file};

/// Тип файла .xlsx (Content-Type ответа и документа Telegram).
pub const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// Книга «Упущено» по данным периода. Принимает: данные (Core::uncapped_report), границы (для подписи периода),
/// время выгрузки. Возвращает: байты .xlsx или текст ошибки.
pub fn missed(
    data: &MissedData,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Vec<u8>, String> {
    missed_xlsx::workbook(data, &period_text(from, to), now).map_err(|e| format!("отчёт xlsx: {e}"))
}

/// Время в периоде [from, to). Ошибок нет.
pub fn in_period(t: DateTime<Utc>, from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> bool {
    from.is_none_or(|f| t >= f) && to.is_none_or(|x| t < x)
}

/// Подпись периода: «за всё время», «с … UTC», «с … по … UTC». Ошибок нет.
pub fn period_text(from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> String {
    let f = |t: DateTime<Utc>| t.format("%Y-%m-%d %H:%M").to_string();
    match (from, to) {
        (None, None) => "за всё время".into(),
        (Some(a), None) => format!("с {} UTC по момент выгрузки", f(a)),
        (None, Some(b)) => format!("до {} UTC", f(b)),
        (Some(a), Some(b)) => format!("с {} по {} UTC", f(a), f(b)),
    }
}

/// Имя файла отчёта: <префикс>_<ГГГГ-ММ-ДД>.xlsx (arb_results, arb_missed). Ошибок нет.
pub fn file_name(prefix: &str, now: DateTime<Utc>) -> String {
    format!("{prefix}_{}.xlsx", now.format("%Y-%m-%d"))
}

/// Файл «Упущено» за период — данные экрана «Упущено из-за потолка», книга в spawn_blocking.
/// Принимает: ядро, границы, время выгрузки. Возвращает: байты .xlsx. Ошибки: текст (база, книга).
pub async fn missed_file(
    core: &Core,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Vec<u8>, String> {
    let data = core
        .uncapped_report(from, to)
        .await
        .map_err(|e| format!("отчёт «Упущено»: записи не прочитаны: {e}"))?;
    tokio::task::spawn_blocking(move || missed(&data, from, to, now))
        .await
        .map_err(|e| format!("отчёт: задача сборки прервана: {e}"))?
}

/// Решение 255: «Сигналы и сделки» за период [from, to) — файлы по одному в `send`. Исходные данные ядра
/// (Core::signals_src: короткие выборки и копии); чтение файлов окон и эпизодов и сборка книг — потоком в spawn_blocking
/// (sig_stream.rs; торговля не ждёт, память не растёт с периодом): следующий файл собирается, пока отправляется
/// предыдущий (канал на один файл). Принимает: ядро, границы, подпись периода, время выгрузки, предел байт файла,
/// отправку файла. Возвращает: сколько файлов отправлено. Ошибки: текст (база, книга, файл не делится до предела, сбой
/// отправки — тогда сборка останавливается).
#[allow(clippy::too_many_arguments)]
pub async fn signals_send<F, Fut>(
    core: &Core,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    period: String,
    now: DateTime<Utc>,
    max: usize,
    mut send: F,
) -> Result<usize, String>
where
    F: FnMut(sig_parts::File) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let src = core
        .signals_src(now)
        .await
        .map_err(|e| format!("отчёт «Сигналы и сделки»: {e}"))?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<sig_parts::File>(1);
    let job = tokio::task::spawn_blocking(move || {
        let notes = sig_notes::notes(&src, &period);
        let prefix = format!("arb_signals_{}", now.format("%Y-%m-%d"));
        let caption = format!("Отчёт «Сигналы и сделки» — {period}");
        let mut emit = |f: sig_parts::File| tx.blocking_send(f).is_ok();
        sig_stream::stream(&src, from, to, max, &prefix, &caption, &notes, &mut emit)
    });
    let mut failed = None;
    while let Some(f) = rx.recv().await {
        if let Err(e) = send(f).await {
            failed = Some(e);
            break;
        }
    }
    // Сбой отправки: приёмник закрыт — сборка остановится на следующем файле.
    drop(rx);
    let built = job
        .await
        .map_err(|e| format!("отчёт: задача сборки прервана: {e}"))?;
    match failed {
        Some(e) => Err(e),
        None => built,
    }
}
