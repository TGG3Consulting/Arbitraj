//! Файл: core/settlement_recovery.rs.
//! Что делает: сохраняет состояние через CAS и продолжает незавершённые шаги после старта.
//! Кто вызывает: движок, исполнитель маршрута и API.
//! От чего зависит: ledger.sqlite и расписание сверки.
//! Настройки: usdt.return_retry_s, usdt.return_retries_max.
use super::{
    engine::Core,
    settlement_state::{Path, Plan},
};
use crate::storage::db::Db;
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
const KIND: &str = "settlement_path";
const LEGACY: &str = "base_settlement_path";
fn key(p: &Plan) -> Result<String, String> {
    serde_json::to_string(&(&p.deal, &p.direction)).map_err(|e| e.to_string())
}

/// Публикует одну версию пути. Конкурентный процесс продолжает с выигравшей версии на следующем входе.
pub(super) async fn save(db: &Db, path: &Path, prev: Option<&Path>) -> Result<(), String> {
    let id = key(&path.plan)?;
    let body = serde_json::to_string(path).map_err(|e| e.to_string())?;
    let previous = prev
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| e.to_string())?;
    let deal = path.plan.deal.clone();
    let n=db.ledger().write(move |c| {
        let now=Utc::now().to_rfc3339();
        let Some(previous)=previous else {
            return c.execute("INSERT INTO core_state(kind,id,body,updated_at)
                SELECT ?1,?2,?3,?4 WHERE NOT EXISTS (
                SELECT 1 FROM core_state WHERE kind IN (?1,?5) AND json_extract(body,'$.plan.deal')=?6)
                ON CONFLICT(kind,id) DO NOTHING",params![KIND,id,body,now,LEGACY,deal]);
        };
        let old=c.query_row("SELECT body FROM core_state WHERE kind IN (?1,?3) AND id=?2",
            params![KIND,id,LEGACY],|r|r.get::<_,String>(0)).optional()?;
        let Some(raw)=old else {return Ok(0)};
        let same=serde_json::from_str::<Path>(&raw).ok().and_then(|p|serde_json::to_string(&p).ok())
            .is_some_and(|s|s==previous);
        if !same{return Ok(0);}
        c.execute("UPDATE core_state SET body=?1,updated_at=?2 WHERE kind IN (?3,?6) AND id=?4 AND body=?5",
            params![body,now,KIND,id,raw,LEGACY])
    }).await.map_err(|e|e.to_string())?;
    if n == 1 {
        Ok(())
    } else {
        Err("Состояние денежного пути уже изменилось; требуется чтение сохранённой версии".into())
    }
}

pub async fn get(db: &Db, p: &Plan) -> Result<Option<Path>, String> {
    let mut possible = rows(db, Some(&p.deal)).await?;
    if possible.len() > 1 {
        return Err("У исходной сделки больше одного сохранённого денежного поручения".into());
    }
    Ok(possible.pop())
}

/// Сохранённые шаги одной сделки для экрана сверки и восстановления после остановки.
pub async fn rows(db: &Db, deal: Option<&str>) -> Result<Vec<Path>, String> {
    let deal = deal.map(String::from);
    let bodies=db.ledger().read(move |c| {
        let mut q=c.prepare("SELECT body FROM core_state WHERE kind IN (?1,?3) AND ((?2 IS NULL AND (json_extract(body,'$.conversion') IS NULL OR json_extract(body,'$.movement') IS NULL)) OR json_extract(body,'$.plan.deal')=?2)")?;
        q.query_map(params![KIND,deal,LEGACY], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()
    }).await.map_err(|e|e.to_string())?;
    bodies
        .into_iter()
        .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
        .collect()
}

/// Архивные отдельные пути не сохраняли идентичность биржевого счёта/кошелька. Доступны через
/// GET архива по ID. Современные поручения с подписантом возобновляет settlement_resume::resume.
pub async fn resume(core: &Core) {
    use crate::storage::journal::Level;
    let paths = match rows(&core.inner.deps.db, None).await {
        Ok(v) => v,
        Err(e) => {
            core.inner
                .deps
                .log(Level::Error, "core.settlement_storage", None, &e, None)
                .await;
            return;
        }
    };
    for path in paths.into_iter().filter(|p| !p.steps_confirmed()) {
        if !super::engine::lk(&core.inner.deals)
            .get(&path.plan.deal)
            .is_some_and(|d| d.plan.get(super::settlement_cycle::FIELD).is_some())
        {
            core.inner.deps.log(Level::Warn, "core.settlement_pending", Some(&path.plan.cex.0),
                format!("{}: у отдельного архивного поручения отсутствует исходная привязка счетов. Для ручной сверки доступны записи по ID сделки",path.plan.deal),None).await;
        }
    }
}
