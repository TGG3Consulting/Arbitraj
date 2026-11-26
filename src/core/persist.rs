//! Файл: core/persist.rs
//! Что делает: учёт ядра в SQLite — сделки, переводы, статистика газа, алерты, ручное управление,
//! итог дня (ТЗ, стек: «Сделки, переводы, учёт — SQLite»). Одна таблица core_state: вид, id, JSON.
//! Решение 353 (Тигран 16.11: «Сделка имеет своё отдельное соединение и ни с кем не конкурирует»): таблица — в своём
//! файле учёта ledger.sqlite (storage/ledger_db.rs): запись — своим писателем в своём потоке ОС, сохранение ждёт только
//! коммита своей строки (запись журнала, окна, отчёта перед ней встать не может — у файла нет других писателей);
//! чтение — своим соединением, писателя не ждёт (WAL). Схема таблицы — storage/ledger_db.rs SCHEMA.
//! Кто вызывает: ledger.rs, gas_stats.rs, manual.rs, transfers.rs, transit.rs, withdraw_seen.rs, low_usdt.rs;
//! ledger_load.rs, daily_migrate.rs, uncapped_recompute_all.rs (load сделок).
//! От чего зависит: storage/db.rs (ledger), storage/ledger_db.rs, serde_json, chrono.
//! Настройки: нет (долговечность записи — ledger.synchronous, применяет storage/write_modes.rs).

use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::storage::db::{Db, DbError};

/// Сохраняет запись (новую или поверх старой) и ждёт её коммита в файле учёта. Принимает: вид, id, значение.
/// Ошибки: база, JSON.
pub async fn save<T: Serialize>(db: &Db, kind: &str, id: &str, v: &T) -> Result<(), DbError> {
    let body = serde_json::to_string(v).map_err(|e| DbError::Json(e.to_string()))?;
    save_body(db, kind, id, body, None, None, None).await
}
/// Кэш общей валюты наблюдает тот же подтверждённый порядок изменений сделки в SQLite.
pub async fn save_deal(db: &Db, d: &super::deal::Deal) -> Result<(), DbError> {
    let body = serde_json::to_string(d).map_err(|e| DbError::Json(e.to_string()))?;
    let cash = d
        .plan
        .get(super::settlement_cycle::FIELD)
        .and_then(|v| v.pointer("/market/cex"))
        .and_then(|v| v.as_str())
        .map(|venue| (venue.to_string(), d.is_open()));
    let opening = d.plan.get(super::cash_order::OPENING).map(|raw| {
        let entry = if d.step == super::deal::Step::Buying {
            serde_json::from_value(raw.clone()).ok()
        } else {
            None
        };
        (d.direction.buy_on.0.clone(), entry)
    });
    let archived = d
        .plan
        .get("cash_legacy_ioc")
        .and_then(|v| v.as_array())
        .map(|rows| {
            (
                d.direction.buy_on.0.clone(),
                rows.iter()
                    .filter_map(|v| v.get("action")?.as_str().map(str::to_owned))
                    .collect::<Vec<_>>(),
            )
        });
    save_body(
        db,
        super::ledger::KIND,
        &d.id,
        body,
        cash,
        opening,
        archived,
    )
    .await
}
async fn save_body(
    db: &Db,
    kind: &str,
    id: &str,
    body: String,
    cash: Option<(String, bool)>,
    opening: Option<(String, Option<crate::storage::cex_funds_watch::Opening>)>,
    archived: Option<(String, Vec<String>)>,
) -> Result<(), DbError> {
    let watch = db.ledger().cash_watch.clone();
    let (kind, id) = (kind.to_string(), id.to_string());
    // Будущее записи — в куче (Box::pin): будущие шагов сделки не растут на его размер.
    let write = db.ledger().write(move |c| {
        c.execute(
            "INSERT INTO core_state (kind, id, body, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(kind, id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
            params![kind, id, body, Utc::now().to_rfc3339()],
        )?;
        if let Some((venue,keys))=archived{watch.archived(&venue,&keys);}
        if let Some((venue,entry))=opening {watch.opening(&venue,&id,entry);}
        if let Some((venue,open))=cash {watch.mandate(&venue,&id,open);}
        Ok(())
    });
    Box::pin(write).await
}

/// Удаляет запись. Ошибки: база.
pub async fn remove(db: &Db, kind: &str, id: &str) -> Result<(), DbError> {
    let (kind, id) = (kind.to_string(), id.to_string());
    db.ledger()
        .write(move |c| {
            c.execute(
                "DELETE FROM core_state WHERE kind = ?1 AND id = ?2",
                params![kind, id],
            )
            .map(|_| ())
        })
        .await
}

/// Все записи вида. Повреждённые строки пропускаются с записью в лог процесса.
/// Ошибки: база.
pub async fn load<T: DeserializeOwned + Send + 'static>(
    db: &Db,
    kind: &str,
) -> Result<Vec<T>, DbError> {
    let kind = kind.to_string();
    let rows: Vec<(String, String)> = db
        .ledger()
        .read(move |c| {
            let mut st = c.prepare("SELECT id, body FROM core_state WHERE kind = ?1")?;
            let it = st.query_map(params![kind], |r| Ok((r.get(0)?, r.get(1)?)))?;
            it.collect()
        })
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, body)| match serde_json::from_str(&body) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::error!(%id, %e, "пропущена повреждённая запись учёта ядра");
                None
            }
        })
        .collect())
}
