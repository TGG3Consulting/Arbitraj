//! Файл: storage/ledger_move.rs
//! Что делает: решение 353 (ревью t158 Б3) — перенос учёта ядра из основной базы в файл учёта (storage/ledger_db.rs)
//! ОДИН раз: таблицы core_state, daily_result, wallet_queue копируются, и в той же транзакции в таблицу meta файла
//! учёта пишется отметка «перенесено» (MOVED). Отметка есть — перенос не делается никогда (основная база больше не
//! источник: строки там устаревают, и сравнение по времени не нужно — переводы часов не влияют). Копируются только
//! строки, которых в файле учёта нет (свежие строки файла учёта не затираются). Основная база не меняется: прежние
//! строки остаются там как были (откат на старую версию — scripts/ledger_rollback.sh, повторный перенос после отката —
//! только явно: scripts/ledger_reimport.sh убирает файл учёта в сторону, следующий запуск переносит заново).
//! Кто вызывает: storage/ledger_db.rs (LedgerDb::open).
//! От чего зависит: rusqlite, chrono.
//! Настройки: нет.

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};

/// Ключ отметки «перенесено» в таблице meta файла учёта.
pub const MOVED: &str = "moved_from_main";

/// Таблицы учёта ядра и их ключевые столбцы (схема — storage/ledger_db.rs SCHEMA и storage/db.rs SCHEMA).
const TABLES: &[(&str, &[&str])] = &[
    ("core_state", &["kind", "id"]),
    ("daily_result", &["day"]),
    ("wallet_queue", &["venue"]),
];

/// Что делает: переносит учёт, если отметки «перенесено» ещё нет; отметка — в той же транзакции. Принимает:
/// соединение основной базы, соединение файла учёта. Возвращает: сколько строк перенесено (0 — уже перенесено или
/// нечего). Ошибки: SQLite (транзакция откатывается целиком, отметки нет — следующий запуск попробует снова).
pub fn from_main(main: &Connection, led: &mut Connection) -> rusqlite::Result<usize> {
    let tx = led.transaction()?;
    let done: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![MOVED],
            |r| r.get(0),
        )
        .optional()?;
    if done.is_some() {
        return Ok(0);
    }
    let mut n = 0;
    for (table, keys) in TABLES {
        n += move_table(main, &tx, table, keys)?;
    }
    tx.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)",
        params![MOVED, format!("{} ({n} строк)", Utc::now().to_rfc3339())],
    )?;
    tx.commit()?;
    tracing::info!(
        n,
        "учёт ядра: перенос из основной базы в ledger.sqlite выполнен"
    );
    Ok(n)
}

/// Перенос одной таблицы: строки, которых в файле учёта нет. Принимает: основную базу, транзакцию файла учёта,
/// таблицу, ключевые столбцы. Возвращает: сколько строк перенесено. Ошибки: SQLite.
fn move_table(
    main: &Connection,
    tx: &Transaction<'_>,
    table: &str,
    keys: &[&str],
) -> rusqlite::Result<usize> {
    let cols = keys.join(", ");
    let marks = (1..=keys.len() + 2)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut st = main.prepare(&format!("SELECT {cols}, body, updated_at FROM {table}"))?;
    let mut put = tx.prepare(&format!(
        "INSERT INTO {table} ({cols}, body, updated_at) VALUES ({marks}) ON CONFLICT({cols}) DO NOTHING"
    ))?;
    let width = keys.len() + 2;
    let mut rows = st.query([])?;
    let mut n = 0;
    while let Some(r) = rows.next()? {
        let vals = (0..width)
            .map(|i| r.get::<_, String>(i))
            .collect::<rusqlite::Result<Vec<String>>>()?;
        n += put.execute(params_from_iter(vals.iter()))?;
    }
    Ok(n)
}
