//! Файл: core/persist.rs
//! Что делает: учёт ядра в SQLite — сделки, переводы, статистика газа, алерты, ручное управление,
//! итог дня (ТЗ, стек: «Сделки, переводы, учёт — SQLite»). Одна таблица core_state: вид, id, JSON.
//! Схема таблицы — в storage/db.rs; ядро пишет и читает через Db::call.
//! Решение 342 (Тигран 16.11: «Сделка имеет своё отдельное соединение и ни с кем не конкурирует»): записи сделок
//! (вид ledger::KIND) при включённой ledger.own_db пишутся своим соединением в свой файл (storage/ledger_db.rs) — ждут
//! только свою запись; загрузка сделок читает оба файла и берёт по каждой сделке запись с последним updated_at
//! (включение и выключение на ходу ничего не теряют, перенос старой базы не нужен).
//! Кто вызывает: ledger.rs, gas_stats.rs, manual.rs, transfers.rs; ledger_load.rs, daily_migrate.rs,
//! uncapped_recompute_all.rs (load сделок).
//! От чего зависит: storage/db.rs, storage/db_ledger.rs (ledger, ledger_rows), serde_json, chrono.
//! Настройки: ledger.own_db, ledger.synchronous (переключает storage/write_modes.rs).

use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::storage::db::{Db, DbError};
use crate::storage::ledger_db::Row;

/// Сохраняет запись (новую или поверх старой). Принимает: вид, id, значение. Ошибки: база, JSON.
pub async fn save<T: Serialize>(db: &Db, kind: &str, id: &str, v: &T) -> Result<(), DbError> {
    let body = serde_json::to_string(v).map_err(|e| DbError::Json(e.to_string()))?;
    // Решение 342: сделка — своим соединением в свой файл (ждётся только эта запись).
    if kind == super::ledger::KIND
        && let Some(l) = db.ledger()
    {
        return l.save(kind, id, body).await;
    }
    let (kind, id) = (kind.to_string(), id.to_string());
    db.call(move |c| {
        c.execute(
            "INSERT INTO core_state (kind, id, body, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(kind, id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
            params![kind, id, body, Utc::now().to_rfc3339()],
        )
        .map(|_| ())
    })
    .await
}

/// Удаляет запись. Ошибки: база.
pub async fn remove(db: &Db, kind: &str, id: &str) -> Result<(), DbError> {
    let (kind, id) = (kind.to_string(), id.to_string());
    db.call(move |c| {
        c.execute(
            "DELETE FROM core_state WHERE kind = ?1 AND id = ?2",
            params![kind, id],
        )
        .map(|_| ())
    })
    .await
}

/// Все записи вида. Повреждённые строки пропускаются с записью в лог процесса.
/// Решение 342: сделки (ledger::KIND) — из основной базы и из файла учёта; по каждому id — строка с последним
/// updated_at. Ошибки: база (основная или файл учёта).
pub async fn load<T: DeserializeOwned + Send + 'static>(
    db: &Db,
    kind: &str,
) -> Result<Vec<T>, DbError> {
    let k = kind.to_string();
    let mut rows: Vec<Row> = db
        .call(move |c| {
            let mut st =
                c.prepare("SELECT id, body, updated_at FROM core_state WHERE kind = ?1")?;
            let it = st.query_map(params![k], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            it.collect()
        })
        .await?;
    if kind == super::ledger::KIND {
        rows = latest(rows, db.ledger_rows(kind).await?);
    }
    Ok(rows
        .into_iter()
        .filter_map(|(id, body, _)| match serde_json::from_str(&body) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::error!(%id, %e, "пропущена повреждённая запись учёта ядра");
                None
            }
        })
        .collect())
}

/// Решение 342: строки двух файлов, по каждому id — с последним updated_at (RFC 3339; не разобралось — строка файла
/// учёта). Принимает: строки основной базы, строки файла учёта. Возвращает: объединение. Ошибок нет.
pub fn latest(main: Vec<Row>, own: Vec<Row>) -> Vec<Row> {
    if own.is_empty() {
        return main;
    }
    let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
    let mut by: std::collections::HashMap<String, Row> =
        main.into_iter().map(|r| (r.0.clone(), r)).collect();
    for r in own {
        let newer = match by.get(&r.0) {
            Some(m) => match (at(&m.2), at(&r.2)) {
                (Some(a), Some(b)) => b >= a,
                _ => true,
            },
            None => true,
        };
        if newer {
            by.insert(r.0.clone(), r);
        }
    }
    by.into_values().collect()
}
