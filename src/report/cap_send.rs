//! Файл: report/cap_send.rs
//! Что делает: решение 256 (с дополнением — аудит отчёта) — отчёт «Сделки, лимиты и капитал»: что прочитать у ядра
//! (gather: площадки DEX, кошельки DEX с адресами, лимиты на момент выгрузки, слепок балансов — та же функция, что у
//! «Итогов сделок», report/snap.rs gather; срок хранения uncapped.keep_days), данные книги (capital_data) и отправка
//! файлов по одному сразу после сборки (capital_send; сборка — в spawn_blocking, торговля не ждёт).
//! Кто вызывает: notify/ctl/report_cap.rs (capital_send), tests/report_capital.rs (capital_data, gather).
//! От чего зависит: core (deals, uncapped_report, настройки, шлюз — адрес кошелька), report/cap_rows.rs,
//! cap_episodes.rs, cap_limits.rs, cap_notes.rs, cap_xlsx.rs, cap_parts.rs, snap.rs, rows_time.rs.
//! Настройки: uncapped.keep_days (читается для примечания); report.snapshot_timeout_ms — у snap.rs; лимиты — cap_limits.rs.

use crate::core::Core;
use crate::core::deal::Deal;
use crate::core::types::DexAccountId;
use crate::core::uncapped_report::MissedData;
use crate::settings::read::At;

use super::cap_limits::LimitRow;
use super::cap_rows::RowsCtx;
use super::rows_time::Window;
use super::snap::Snapshot;
use super::{cap_episodes, cap_notes, cap_rows, cap_xlsx};

/// Прочитанное у ядра для книги (кроме сделок и записей «без потолка»).
#[derive(Debug, Clone)]
pub struct Inputs {
    pub rows: RowsCtx,
    pub limits: Vec<LimitRow>,
    pub snap: Snapshot,
    /// uncapped.keep_days на момент выгрузки (Err — причина).
    pub keep_days: Result<u64, String>,
}

/// Что делает: читает у ядра всё для книги (см. шапку). Принимает: ядро, сделки (площадки DEX сделок). Возвращает:
/// входные данные. Ошибок нет: что не прочитано — причиной в данных.
pub async fn gather(core: &Core, deals: &[Deal]) -> Inputs {
    let s = &core.inner.deps.settings;
    let mut rows = RowsCtx::default();
    for d in deals {
        for v in [&d.direction.buy_on.0, &d.direction.sell_on.0] {
            if crate::core::daily_venue::is_dex(core, v) {
                rows.dex.insert(v.clone());
            }
        }
    }
    let dex = core.dex_id();
    for w in s.accounts().wallets() {
        rows.names.insert(w.id.clone(), w.name.clone());
        let addr = match &dex {
            None => Err("площадка DEX не подключена".to_string()),
            Some(v) => match core
                .inner
                .deps
                .gw
                .get_as(v, Some(&DexAccountId(w.id.clone())))
                .await
            {
                Ok(g) => g.wallet_address().await.map_err(|e| e.message),
                Err(e) => Err(e.message),
            },
        };
        rows.wallets.insert(w.id, addr);
    }
    let kinds = crate::core::engine::rd(&core.inner.kinds).clone();
    Inputs {
        rows,
        limits: super::cap_limits::rows(s, &kinds),
        snap: super::snap::gather(core).await,
        keep_days: s
            .integer("uncapped.keep_days", At::default())
            .map_err(|e| e.message),
    }
}

/// Решение 256: данные книги за окно: сделки учёта ядра (cap_rows.rs), эпизоды «без потолка» (те же записи, что у
/// экрана и отчёта «Упущено», Core::uncapped_report), лимиты, слепок, примечания. Принимает: сделки, данные «Упущено»,
/// окно, подпись периода, прочитанное у ядра. Возвращает: данные книги. Ошибок нет.
pub fn capital_data(
    deals: &[Deal],
    missed: &MissedData,
    w: &Window,
    period: &str,
    x: &Inputs,
) -> cap_xlsx::Data {
    let ladder: Vec<rust_decimal::Decimal> = missed.totals.ladder.iter().map(|x| x.0).collect();
    let eps = cap_episodes::episodes(&missed.totals, &ladder);
    let notes = cap_notes::notes(&cap_notes::NotesIn {
        window: w,
        incomparable: eps.incomparable,
        ladder_error: missed.ladder_error.as_deref(),
        off: missed.off.as_deref(),
        keep_days: x.keep_days.clone(),
    });
    let mut limit_notes = vec![
        format!(
            "Значения — на момент выгрузки {} UTC (не на момент каждой сделки).",
            w.now.format("%Y-%m-%d %H:%M:%S")
        ),
        "Пары — все площадки реестра шлюза: покупка → продажа; DEX → та же DEX — цикл DEX ↔ DEX."
            .to_string(),
    ];
    if !x
        .limits
        .iter()
        .any(|r| r.what.contains('→') || r.what.contains('↔'))
    {
        limit_notes
            .push("Реестр площадок пуст — пар нет, показаны только значения «вся система».".into());
    }
    cap_xlsx::Data {
        deals: cap_rows::rows(deals, w, &x.rows),
        days: eps.rows,
        ladder,
        title: format!("Сделки, лимиты и капитал — {period}"),
        notes,
        limits: x.limits.clone(),
        limit_notes,
        snap: x.snap.clone(),
        at: w.now.format("%Y-%m-%d %H:%M:%S").to_string(),
    }
}

/// Решение 256: «Сделки, лимиты и капитал» за окно — файлы по одному в `send`. Данные ядра читаются коротко (копия учёта
/// сделок, записи «без потолка» из базы, лимиты, слепок); сборка книг — в spawn_blocking (торговля не ждёт); каждый
/// файл отдаётся, как только собран (канал на один файл). Принимает: ядро, окно, подпись периода, предел байт файла,
/// отправку файла. Возвращает: сколько файлов отправлено. Ошибки: текст (база, книга, день больше предела, сбой
/// отправки — тогда сборка останавливается).
pub async fn capital_send<F, Fut>(
    core: &Core,
    w: Window,
    period: String,
    max: usize,
    mut send: F,
) -> Result<usize, String>
where
    F: FnMut(cap_xlsx::File) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    // Записи «без потолка» — с начала окна до выгрузки (окно to — граница своего интервала).
    let missed = core.uncapped_report(w.from, w.to).await.map_err(|e| {
        format!("отчёт «Сделки, лимиты и капитал»: записи «без потолка» не прочитаны: {e}")
    })?;
    let deals = core.deals();
    let inputs = gather(core, &deals).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<cap_xlsx::File>(1);
    let job = tokio::task::spawn_blocking(move || {
        let data = capital_data(&deals, &missed, &w, &period, &inputs);
        drop(deals);
        let prefix = format!("arb_capital_{}", w.now.format("%Y-%m-%d"));
        let caption = format!(
            "Отчёт «Сделки, лимиты и капитал» — {period}; окно {}",
            w.text()
        );
        let mut emit = |f: cap_xlsx::File| tx.blocking_send(f).is_ok();
        super::cap_parts::files(&data, max, &prefix, &caption, &mut emit)
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
