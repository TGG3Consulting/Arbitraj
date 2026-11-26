//! Стойкий идентификатор денежного шага Base. Адаптер вызывает before_send с реальным client ID
//! или хешем подписанной транзакции непосредственно перед POST. Повтор того же логического шага
//! возвращает исходный идентификатор для сверки. За пределами связанного шага поведение прежнее.
use crate::{
    core::types::{TradeRequest, TradeResult, Transfer, WithdrawRequest},
    gateway::{
        errors::{ErrorCode, GatewayError, GatewayResult},
        pending::PendingRef,
    },
    storage::db::Db,
};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::future::Future;

const KIND: &str = "base_settlement_action";

/// Действительный запрос к площадке, отдельный для ордера и вывода.
#[derive(Clone, Serialize, Deserialize)]
pub enum Request {
    Order(Box<TradeRequest>),
    Withdrawal(Box<WithdrawRequest>),
}

/// Итог принятого действия. У вывода это запись отправителя; зачисление подтверждается отдельно.
#[derive(Clone, Serialize, Deserialize)]
pub enum Outcome {
    Order(Box<TradeResult>),
    Withdrawal(Box<Transfer>),
}

/// Сохраняется вместе с реальным идентификатором до первого запроса. Ошибка результата не стирает намерение.
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub operation: Request,
    pub lookup: PendingRef,
    pub result: Option<Outcome>,
}

#[derive(Clone)]
struct Action {
    db: Db,
    key: String,
    operation: Request,
}

tokio::task_local! { static ACTION: Action; }

/// Требуется ли этой операции сохранение идентификатора до отправки.
pub fn active() -> bool {
    ACTION.try_with(|_| ()).is_ok()
}

/// Связывает один денежный шаг с устойчивым ключом вида «сделка + этап».
pub async fn scope<T>(db: &Db, key: &str, operation: Request, f: impl Future<Output = T>) -> T {
    ACTION
        .scope(
            Action {
                db: db.clone(),
                key: key.into(),
                operation,
            },
            f,
        )
        .await
}

fn error(text: impl Into<String>) -> GatewayError {
    GatewayError::new(ErrorCode::Internal, None, text)
}

/// Загружает результат/ссылку для восстановления после остановки, не отправляя запрос на площадку.
pub async fn load(db: &Db, key: &str) -> Result<Option<Entry>, String> {
    let id = key.to_owned();
    let row = db
        .ledger()
        .read(move |c| {
            c.query_row(
                "SELECT body FROM core_state WHERE kind=?1 AND id=?2",
                params![KIND, id],
                |row| row.get::<_, String>(0),
            )
            .optional()
        })
        .await
        .map_err(|e| e.to_string())?;
    row.map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| e.to_string())
}

/// Непосредственно перед внешней отправкой сохраняет реальный prepared ID. Только выигравший INSERT может отправить.
pub async fn before_send(lookup: &PendingRef) -> GatewayResult<()> {
    let Ok(action) = ACTION.try_with(Clone::clone) else {
        return Ok(());
    };
    let body = serde_json::to_string(&Entry {
        operation: action.operation,
        lookup: lookup.clone(),
        result: None,
    })
    .map_err(|e| error(e.to_string()))?;
    let id = action.key;
    let db = action.db;
    let key = id.clone();
    let inserted = db.ledger().write(move |c| {
        c.execute("INSERT INTO core_state(kind,id,body,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(kind,id) DO NOTHING",
            params![KIND,key,body,Utc::now().to_rfc3339()])
    }).await.map_err(|e| error(e.to_string()))?;
    if inserted == 1 {
        return Ok(());
    }
    let prior = load(&db, &id)
        .await
        .map_err(error)?
        .ok_or_else(|| error("Исчезла запись денежного шага Base"))?;
    Err(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        "Этот шаг Base уже зарегистрирован: требуется сверка сохранённого идентификатора",
    )
    .pending(prior.lookup))
}

/// Записывает подтверждённый биржей/сетью итог. Сначала проверяет, что сохранён тот же подготовленный шаг.
pub async fn confirm(db: &Db, key: &str, original: &Entry, outcome: Outcome) -> Result<(), String> {
    let expected = serde_json::to_string(original).map_err(|e| e.to_string())?;
    let mut entry = original.clone();
    entry.result = Some(outcome);
    let body = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
    let key = key.to_string();
    let count =
        db.ledger()
            .write(move |c| {
                c.execute(
        "UPDATE core_state SET body=?1,updated_at=?2 WHERE kind=?3 AND id=?4 AND body=?5",
        params![body,Utc::now().to_rfc3339(),KIND,key,expected])
            })
            .await
            .map_err(|e| e.to_string())?;
    if count == 1 {
        Ok(())
    } else {
        Err("Шаг Base изменился во время сверки результата".into())
    }
}
