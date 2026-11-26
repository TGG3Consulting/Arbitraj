//! Те же четыре книги, что и в меню управления. Окно всех файлов закреплено за сроком UTC.
use super::spool::Spool;
use crate::notify::tg_methods::DOCUMENT_MAX_BYTES;
use crate::{
    core::Core,
    report::{self, archive::Archive, rows_time::Window, sig_parts::File, snap::Snapshot},
};
use chrono::{DateTime, TimeDelta, Utc};

pub(super) fn window(slot: DateTime<Utc>, now: DateTime<Utc>) -> Window {
    Window {
        from: Some(slot - TimeDelta::hours(24)),
        to: Some(slot),
        now,
    }
}

pub(super) async fn build(
    core: &Core,
    archive: &Archive,
    spool: &Spool,
    report: &'static str,
    w: Window,
    snapshot: &mut Option<Snapshot>,
) -> Result<(), String> {
    let period = w.text();
    if matches!(report, "res" | "cap") && snapshot.is_none() {
        *snapshot = Some(report::snap_cached::gather(core, &archive.deals, w.now).await);
    }
    let day = w.to.unwrap_or(w.now).date_naive();
    let save = |mut f: File| {
        // Имя — дата назначенного срока, даже если бот восстановился после полуночи.
        if w.now.date_naive() != day {
            f.name = f
                .name
                .replacen(&w.now.date_naive().to_string(), &day.to_string(), 1);
        }
        spool.append(report, f)
    };
    match report {
        "res" | "mis" => {
            let bytes = if report == "res" {
                report::results::results_with_snapshot(
                    core,
                    w,
                    Some((archive, snapshot.clone().expect("слепок итогов"))),
                )
                .await?
            } else {
                report::missed_file_archive(core, archive, w.from, w.to, w.now).await?
            };
            let (_, title, prefix) = super::super::ctl::report::REPORTS
                .iter()
                .find(|r| r.0 == report)
                .expect("отчёт меню");
            save(File {
                bytes,
                name: report::file_name(prefix, w.now),
                caption: format!("Отчёт «{title}» — {period}"),
            })
            .await?;
        }
        "sig" => {
            report::signals_send_archive(
                core,
                archive,
                w.from,
                w.to,
                period,
                w.now,
                DOCUMENT_MAX_BYTES,
                save,
            )
            .await?;
        }
        "cap" => {
            report::cap_send::capital_with_snapshot(
                core,
                w,
                period,
                DOCUMENT_MAX_BYTES,
                Some((archive, snapshot.clone().expect("слепок капитала"))),
                |f| {
                    save(File {
                        name: f.name,
                        caption: f.caption,
                        bytes: f.bytes,
                    })
                },
            )
            .await?;
        }
        _ => return Err(format!("неизвестный отчёт {report}")),
    }
    spool.seal(report).await
}
