//! Файл: report/cap_send.rs
//! Что делает: решение 256 — отчёт «Сделки, лимиты и капитал»: данные книги из учёта сделок ядра и записей «без
//! потолка» периода (capital_data) и отправка файлов по одному по мере сборки (capital_send; сборка — в spawn_blocking).
//! Кто вызывает: notify/ctl/report_cap.rs (capital_send), tests/report_capital.rs (capital_data).
//! От чего зависит: core (deals, uncapped_report), report/cap_rows.rs, cap_episodes.rs, cap_xlsx.rs.
//! Настройки: нет (потолки uncapped.ladder_usdt — через Core::uncapped_report).

use chrono::{DateTime, Utc};

use crate::core::Core;
use crate::core::deal::Deal;
use crate::core::uncapped_report::MissedData;

use super::{cap_episodes, cap_rows, cap_xlsx};

/// Решение 256: данные книги «Сделки, лимиты и капитал» за период [from, to): сделки учёта ядра (по началу) и эпизоды
/// «без потолка» (те же записи, что у экрана и отчёта «Упущено», Core::uncapped_report). Принимает: сделки, данные
/// «Упущено», подпись периода, время выгрузки. Возвращает: данные книги. Ошибок нет.
pub fn capital_data(
    deals: &[Deal],
    missed: &MissedData,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    period: &str,
    now: DateTime<Utc>,
) -> cap_xlsx::Data {
    let ladder: Vec<rust_decimal::Decimal> = missed.totals.ladder.iter().map(|x| x.0).collect();
    let eps = cap_episodes::episodes(&missed.totals, &ladder);
    let mut notes = vec![
        format!("Выгрузка: {} UTC.", now.format("%d.%m.%y %H:%M:%S")),
        "Сделки — все, начатые в периоде; «покупка состоялась» — вложено > 0; конец — последнее обновление записи \
         сделки (с возвратом USDT); у открытой сделки конца нет."
            .into(),
        "«Одновременно открыто в момент начала» — сделки того же направления с покупкой, начатые не позже этой и не \
         закончившиеся к её началу (включая её саму)."
            .into(),
        "Эпизод «без потолка» = направление (монета + биржи) + начало эпизода; по эпизоду — наибольшая прибыль на \
         нашем бюджете и на каждом лимите (модель); «что ограничило» — у пиковой записи эпизода."
            .into(),
    ];
    if eps.incomparable > 0 {
        notes.push(format!(
            "Записи сделок «без потолка» не по данным момента входа (решение 239) не входят: {}.",
            eps.incomparable
        ));
    }
    if let Some(e) = &missed.ladder_error {
        notes.push(format!(
            "Лимиты для модели не заданы: {e} — столбцов лимитов нет."
        ));
    }
    if let Some(e) = &missed.off {
        notes.push(format!("Расчёт «без потолка» сейчас не идёт: {e}."));
    }
    cap_xlsx::Data {
        deals: cap_rows::rows(deals, from, to),
        days: eps.rows,
        ladder,
        title: format!("Сделки, лимиты и капитал — {period}"),
        notes,
    }
}

/// Решение 256: «Сделки, лимиты и капитал» за период [from, to) — файлы по одному в `send`. Данные ядра читаются
/// коротко (копия учёта сделок, записи «без потолка» из базы); сборка книг — отдельной задачей в spawn_blocking (торговля
/// не ждёт); файл отдаётся, как только собран (канал на один файл). Принимает: ядро, границы, подпись периода, время
/// выгрузки, предел байт файла, отправку файла. Возвращает: сколько файлов отправлено. Ошибки: текст (база, книга,
/// день больше предела, сбой отправки — тогда сборка останавливается).
#[allow(clippy::too_many_arguments)]
pub async fn capital_send<F, Fut>(
    core: &Core,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    period: String,
    now: DateTime<Utc>,
    max: usize,
    mut send: F,
) -> Result<usize, String>
where
    F: FnMut(cap_xlsx::File) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let missed = core.uncapped_report(from, to).await.map_err(|e| {
        format!("отчёт «Сделки, лимиты и капитал»: записи «без потолка» не прочитаны: {e}")
    })?;
    let deals = core.deals();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<cap_xlsx::File>(1);
    let job = tokio::task::spawn_blocking(move || {
        let data = capital_data(&deals, &missed, from, to, &period, now);
        drop(deals);
        let prefix = format!("arb_capital_{}", now.format("%Y-%m-%d"));
        let caption = format!("Отчёт «Сделки, лимиты и капитал» — {period}");
        let mut emit = |f: cap_xlsx::File| tx.blocking_send(f).is_ok();
        cap_xlsx::files(&data, max, &prefix, &caption, &mut emit)
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
