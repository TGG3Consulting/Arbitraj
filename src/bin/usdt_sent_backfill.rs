//! Файл: bin/usdt_sent_backfill.rs
//! Что делает: решение 236, дополнение п. 5 (техлид 09.11 по поручению Тиграна) — отдельная программа: один раз дописать
//! «ушло» (sent) в СТАРЫЕ части возврата USDT «вернулись» по записям журнала core.transfer_sent (логика —
//! core/usdt_sent_backfill.rs). Повод: у записей до решения 236 затраты возврата = запрошено − зачислено, вместе с
//! остатком округления вывода, который лежит на площадке-отправителе (MEXC, 145 возвратов, 6.34 USDT).
//! Режимы: --dry-run (по умолчанию) — только отчёт, база открывается на чтение (бот может работать); --apply — отчёт и
//! запись одной транзакцией. При --apply база открывается с исключительной блокировкой (storage/db.rs open_existing):
//! бот запущен — отказ «база занята», остановите бота. Перед --apply — копия базы (инструкция).
//! Отчёт — stdout: каждая часть (сделка, откуда → куда, запрошено, ушло, зачислено, затраты было → станет), пропуски с
//! причиной, итоги по площадкам.
//! Запуск и порядок действий на сервере — docs/notes/usdt-sent-backfill-2026-11-09.md.
//! Аргументы: --data-dir <папка данных бота> (обязательно; база <data-dir>/arb.sqlite), --from <площадка> (можно
//! несколько — только части с этих площадок; без него — все), --apply, --dry-run.
//! Кто вызывает: человек или серверный ИИ (командная строка).
//! От чего зависит: core/usdt_sent_backfill.rs, storage/db.rs.
//! Настройки: нет.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use rust_decimal::Decimal;

use arb::core::usdt_sent_backfill::{self as bf, Fix};
use arb::storage::db::{Db, DbError};

/// Аргументы.
struct Args {
    data_dir: PathBuf,
    from: Vec<String>,
    apply: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Вся работа. Ошибки: текст (аргументы, база занята, база).
async fn run() -> Result<(), String> {
    let a = args(std::env::args().skip(1))?;
    let path = a.data_dir.join("arb.sqlite");
    if !path.is_file() {
        return Err(format!("базы {} нет", path.display()));
    }
    let db = Db::open_existing(&path, a.apply).map_err(|e| busy(e, a.apply))?;
    let (mut deals, sents) = bf::load(&db).await.map_err(|e| format!("база: {e}"))?;
    let (fixes, skips) = bf::plan(&deals, &sents, &a.from);
    for f in &fixes {
        println!(
            "{} {} → {}: запрошено {}, ушло {}, зачислено {}; затраты {} → {} (остаток {} на {}; журнал {})",
            f.deal,
            f.from,
            f.to,
            f.amount,
            f.sent,
            f.credited,
            f.amount - f.credited,
            f.sent - f.credited,
            f.amount - f.sent,
            f.from,
            f.at
        );
    }
    for s in &skips {
        println!(
            "ПРОПУСК {} {} → {} ({}): {}",
            s.deal, s.from, s.to, s.amount, s.why
        );
    }
    summary(&fixes, skips.len());
    let mut rows = vec![];
    for (id, body) in deals.iter_mut() {
        // Каждое исправление сделки применяется (без короткого замыкания: у сделки бывает несколько частей).
        let mut changed = false;
        for f in fixes.iter().filter(|f| &f.deal == id) {
            changed |= bf::apply(body, f);
        }
        if changed {
            rows.push((id.clone(), body.to_string()));
        }
    }
    if !a.apply {
        println!(
            "Режим --dry-run: в базу ничего не записано (сделок к записи было бы {}).",
            rows.len()
        );
        return Ok(());
    }
    let n = bf::write(&db, rows)
        .await
        .map_err(|e| format!("запись в базу: {e}"))?;
    println!(
        "Режим --apply: записано сделок {n}. Итог сделок бот пересчитает при запуске; итоги прошлых дней не меняются."
    );
    Ok(())
}

/// Итоги по площадкам: частей, затраты было (запрошено − зачислено) и станет (ушло − зачислено). Ошибок нет.
fn summary(fixes: &[Fix], skipped: usize) {
    let mut by: BTreeMap<&str, (usize, Decimal, Decimal)> = BTreeMap::new();
    for f in fixes {
        let e = by.entry(f.from.as_str()).or_default();
        e.0 += 1;
        e.1 += f.amount - f.credited;
        e.2 += f.sent - f.credited;
    }
    println!();
    for (v, (n, was, now)) in &by {
        println!(
            "{v}: частей {n}, затраты возврата {} → {} USDT (остаток округления {} — на {v}, не затрата)",
            was.round_dp(4),
            now.round_dp(4),
            (*was - *now).round_dp(4)
        );
    }
    println!("Пропущено частей: {skipped}");
}

/// Разбор аргументов. Ошибки: текст.
fn args(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let (mut data_dir, mut from, mut apply) = (None, vec![], false);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data-dir" => data_dir = it.next().map(PathBuf::from),
            "--from" => from.push(it.next().ok_or("--from без площадки")?),
            "--apply" => apply = true,
            "--dry-run" => apply = false,
            other => return Err(format!("неизвестный аргумент {other}")),
        }
    }
    Ok(Args {
        data_dir: data_dir.ok_or("нужен --data-dir <папка данных бота>")?,
        from,
        apply,
    })
}

/// Текст ошибки открытия базы: занята — понятная причина. Ошибок нет.
fn busy(e: DbError, apply: bool) -> String {
    let locked = matches!(&e, DbError::Sqlite(s) if matches!(
        s.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    ));
    if locked && apply {
        return "база занята — бот запущен (или базу держит другой процесс). Остановите бота и повторите --apply; \
                отчёт без записи — --dry-run"
            .into();
    }
    format!("база: {e}")
}
