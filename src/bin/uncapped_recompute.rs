//! Файл: bin/uncapped_recompute.rs
//! Что делает: решение 218 (Тигран, 07.11.2026) — отдельная программа: один раз пересчитать СТАРЫЕ записи «без
//! потолка» по исполненным сделкам, посчитанные после нашей покупки (basis live_after_buy — занижены, решение 217), по
//! записанным окнам сделок. Модель и поиск — те же функции ядра, что у бота (core/uncapped_recompute.rs → costs.rs
//! evaluate_with → size_uncapped.rs); второй модели нет.
//! Режимы: --dry-run (по умолчанию) — только отчёт, база открывается на чтение (бот может работать); --apply — отчёт
//! и запись новых строк uncapped одной транзакцией (старые не удаляются и не меняются). При --apply база открывается
//! с исключительной блокировкой (storage/db.rs open_existing): бот запущен — отказ «база занята», остановите бота.
//! Отчёт — в stdout (итоги по группам) и в CSV (--report, по умолчанию <data-dir>/uncapped_recompute_<время>.csv).
//! Запуск и порядок действий на сервере — docs/notes/uncapped-recompute-2026-11-07.md.
//! Аргументы: --data-dir <папка данных бота> (обязательно; база <data-dir>/arb.sqlite), --windows-dir <папка файлов
//! окон> (по умолчанию <data-dir>/windows), --report <файл.csv>, --deal <id> (можно несколько — только эти сделки),
//! --apply, --dry-run.
//! Кто вызывает: человек или серверный ИИ (командная строка).
//! От чего зависит: core (uncapped_recompute*), report/recompute.rs, storage (db, uncapped_db), settings (Store),
//! адаптеры бирж — разбор кадров окна (venues/*/tap.rs decode, как Venue::decode_tap).
//! Настройки: модели — из базы бота (core/uncapped_recompute_cfg.rs, uncapped_recompute_ctx.rs).

use std::path::PathBuf;
use std::process::ExitCode;

use tokio::sync::broadcast;

use arb::core::types::{TapDecoded, VenueId};
use arb::core::uncapped_recompute::Env;
use arb::core::uncapped_recompute_all::{all, load};
use arb::gateway::tap::{TapData, TapStream};
use arb::settings::store::Store;
use arb::storage::db::{Db, DbError};
use arb::storage::journal::Journal;
use arb::storage::uncapped_db;

/// Аргументы.
struct Args {
    data_dir: PathBuf,
    windows_dir: Option<PathBuf>,
    report: Option<PathBuf>,
    deals: Vec<String>,
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

/// Весь пересчёт. Ошибки: текст (аргументы, база занята, база, файл отчёта).
async fn run() -> Result<(), String> {
    let a = args(std::env::args().skip(1))?;
    let db_path = a.data_dir.join("arb.sqlite");
    if !db_path.is_file() {
        return Err(format!("базы {} нет", db_path.display()));
    }
    let db = Db::open_existing(&db_path, a.apply).map_err(|e| busy(e, a.apply))?;
    let (changes, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes, journal)
        .await
        .map_err(|e| format!("настройки: {e}"))?;
    let base = load(&db).await?;
    let windows_dir = a
        .windows_dir
        .clone()
        .unwrap_or_else(|| a.data_dir.join("windows"));
    let env = Env {
        settings: &settings,
        windows_dir: &windows_dir,
        decode: &decode,
    };
    let items = all(&env, &base, &a.deals).await;
    let lines: Vec<_> = items.iter().map(|i| i.line.clone()).collect();
    let stamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let report = a
        .report
        .clone()
        .unwrap_or_else(|| a.data_dir.join(format!("uncapped_recompute_{stamp}.csv")));
    std::fs::write(&report, arb::report::recompute::csv(&lines))
        .map_err(|e| format!("отчёт {}: {e}", report.display()))?;
    for l in &lines {
        let tail = match &l.reason {
            Some(r) => format!("не пересчитано: {r}"),
            None => format!(
                "упущено {} → {}, потолок {} → {}",
                show(l.was_missed),
                show(l.now_missed),
                show(l.was_cap),
                show(l.now_cap)
            ),
        };
        println!("{} {} [{}] {tail}", l.deal_id, l.direction, l.group);
    }
    println!("\n{}", arb::report::recompute::summary(&lines));
    println!("Отчёт CSV: {}", report.display());
    let rows: Vec<_> = items.into_iter().filter_map(|i| i.row).collect();
    if !a.apply {
        println!(
            "Режим --dry-run: в базу ничего не записано (новых записей было бы {}).",
            rows.len()
        );
        return Ok(());
    }
    let n = rows.len();
    let ids = uncapped_db::insert_all(&db, rows)
        .await
        .map_err(|e| format!("запись в базу: {e}"))?;
    println!(
        "Режим --apply: дописано записей {n} (id {:?}); старые записи не изменены.",
        ids
    );
    Ok(())
}

/// Разбор кадра окна адаптером своей биржи (тот же код, что Venue::decode_tap). Ошибки: текст.
fn decode(v: &VenueId, s: TapStream, d: &TapData) -> Result<TapDecoded, String> {
    use arb::venues::{binance, bingx, bybit, gate, kucoin, mexc};
    let r = match v.0.as_str() {
        bingx::ctx::ID => bingx::tap::decode(s, d),
        mexc::ctx::ID => mexc::tap::decode(s, d),
        gate::ctx::ID => gate::tap::decode(s, d),
        bybit::ctx::ID => bybit::tap::decode(s, d),
        kucoin::ctx::ID => kucoin::tap::decode(s, d),
        binance::ctx::ID => binance::tap::decode(s, d),
        other => return Err(format!("у площадки {other} нет записи окон")),
    };
    r.map_err(|e| e.message)
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

/// Число для вывода (2 знака; нет — «—»). Ошибок нет.
fn show(v: Option<rust_decimal::Decimal>) -> String {
    v.map(|x| x.round_dp(2).to_string())
        .unwrap_or_else(|| "—".into())
}

/// Разбор аргументов. Ошибки: текст — неизвестный аргумент, нет значения, нет --data-dir.
fn args(it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut it = it.peekable();
    let (mut data_dir, mut windows_dir, mut report, mut deals) = (None, None, None, vec![]);
    let (mut apply, mut dry) = (false, false);
    while let Some(k) = it.next() {
        let mut val = || it.next().ok_or(format!("у {k} нет значения"));
        match k.as_str() {
            "--data-dir" => data_dir = Some(PathBuf::from(val()?)),
            "--windows-dir" => windows_dir = Some(PathBuf::from(val()?)),
            "--report" => report = Some(PathBuf::from(val()?)),
            "--deal" => deals.push(val()?),
            "--apply" => apply = true,
            "--dry-run" => dry = true,
            other => {
                return Err(format!(
                    "неизвестный аргумент {other} (см. docs/notes/uncapped-recompute-2026-11-07.md)"
                ));
            }
        }
    }
    if apply && dry {
        return Err("--apply и --dry-run вместе — выберите один режим".into());
    }
    Ok(Args {
        data_dir: data_dir.ok_or("нужен --data-dir <папка данных бота>")?,
        windows_dir,
        report,
        deals,
        apply,
    })
}
