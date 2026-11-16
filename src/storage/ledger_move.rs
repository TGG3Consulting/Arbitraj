//! Файл: storage/ledger_move.rs
//! Что делает: решение 353 — перенос учёта ядра из основной базы в файл учёта (storage/ledger_db.rs) при открытии.
//! Таблицы core_state, daily_result, wallet_queue: строка копируется, если её нет в файле учёта или в основной базе
//! она новее (updated_at, RFC 3339; не разбирается — не копируется поверх существующей). Основная база не меняется:
//! прежние строки остаются там как были (откат на старую версию — docs/DECISIONS.md, решение 353). Сначала читаются
//! только ключи и updated_at (быстро даже при многих строках), тела — только у переносимых строк; запись в файл
//! учёта — одной транзакцией. Первый запуск новой версии переносит всё; дальше — ничего (или то, что старая версия
//! записала после отката).
//! Кто вызывает: storage/ledger_db.rs (LedgerDb::open).
//! От чего зависит: rusqlite, chrono.
//! Настройки: нет.

use rusqlite::{Connection, OptionalExtension, Transaction, params_from_iter};

/// Таблицы учёта ядра и их ключевые столбцы (схема — storage/ledger_db.rs SCHEMA и storage/db.rs SCHEMA).
const TABLES: &[(&str, &[&str])] = &[
    ("core_state", &["kind", "id"]),
    ("daily_result", &["day"]),
    ("wallet_queue", &["venue"]),
];

/// Что делает: переносит недостающие и более новые строки всех таблиц учёта. Принимает: соединение основной базы,
/// соединение файла учёта. Возвращает: сколько строк перенесено. Ошибки: SQLite (транзакция откатывается целиком).
pub fn from_main(main: &Connection, led: &mut Connection) -> rusqlite::Result<usize> {
    let tx = led.transaction()?;
    let mut n = 0;
    for (table, keys) in TABLES {
        n += move_table(main, &tx, table, keys)?;
    }
    tx.commit()?;
    if n > 0 {
        tracing::info!(
            n,
            "учёт ядра: строки перенесены из основной базы в ledger.sqlite"
        );
    }
    Ok(n)
}

/// Перенос одной таблицы. Принимает: основную базу, транзакцию файла учёта, таблицу, ключевые столбцы.
/// Возвращает: сколько строк перенесено. Ошибки: SQLite.
fn move_table(
    main: &Connection,
    tx: &Transaction<'_>,
    table: &str,
    keys: &[&str],
) -> rusqlite::Result<usize> {
    let cols = keys.join(", ");
    let by_key = keys
        .iter()
        .enumerate()
        .map(|(i, k)| format!("{k} = ?{}", i + 1))
        .collect::<Vec<_>>()
        .join(" AND ");
    // Ключи и время всех строк основной базы (без тел).
    let mut st = main.prepare(&format!("SELECT {cols}, updated_at FROM {table}"))?;
    let all = st
        .query_map([], |r| {
            let key = (0..keys.len())
                .map(|i| r.get::<_, String>(i))
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok((key, r.get::<_, String>(keys.len())?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut have = tx.prepare(&format!("SELECT updated_at FROM {table} WHERE {by_key}"))?;
    let mut body = main.prepare(&format!("SELECT body FROM {table} WHERE {by_key}"))?;
    let marks = (1..=keys.len() + 2)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut put = tx.prepare(&format!(
        "INSERT INTO {table} ({cols}, body, updated_at) VALUES ({marks})
         ON CONFLICT({cols}) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at"
    ))?;
    let mut n = 0;
    for (key, at) in all {
        let led_at: Option<String> = have
            .query_row(params_from_iter(key.iter()), |r| r.get(0))
            .optional()?;
        if !newer(&at, led_at.as_deref()) {
            continue;
        }
        let b: String = body.query_row(params_from_iter(key.iter()), |r| r.get(0))?;
        let mut vals = key;
        vals.push(b);
        vals.push(at);
        put.execute(params_from_iter(vals.iter()))?;
        n += 1;
    }
    Ok(n)
}

/// Копировать ли строку основной базы: в файле учёта её нет, или она там старее. Принимает: updated_at основной базы,
/// updated_at файла учёта. Возвращает: да/нет. Ошибок нет (время не разбирается — поверх существующей не копируем).
fn newer(main_at: &str, led_at: Option<&str>) -> bool {
    let Some(l) = led_at else {
        return true;
    };
    let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
    matches!((at(main_at), at(l)), (Some(m), Some(l)) if m > l)
}
