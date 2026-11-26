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

pub(super) const KIND: &str = "settlement_action";
pub(super) const LEGACY: &str = "base_settlement_action";

/// Действительный запрос к площадке, отдельный для ордера и вывода.
#[derive(Clone, Serialize, Deserialize)]
pub enum Request {
    Order(Box<TradeRequest>),
    Withdrawal(Box<WithdrawRequest>),
}

/// Фактические объёмы ордера по данным биржи. gross_quote — точное поле filled_total.
#[derive(Clone, Serialize, Deserialize)]
pub struct SpotFill {
    pub trade: TradeResult,
    pub gross_quote: rust_decimal::Decimal,
}

tokio::task_local! { static GROSS: std::cell::Cell<Option<rust_decimal::Decimal>>; }

pub fn capturing() -> bool {
    GROSS.try_with(|_| ()).is_ok()
}

/// Адаптер передаёт точную сумму расчётной валюты из окончательного ответа биржи.
pub fn note_gross(value: Option<rust_decimal::Decimal>) {
    let _ = GROSS.try_with(|cell| cell.set(value));
}

/// Извлекает итог того же ответа, которым подтверждено количество и комиссия ордера.
pub async fn capture(
    f: impl Future<Output = GatewayResult<TradeResult>>,
) -> GatewayResult<SpotFill> {
    GROSS
        .scope(std::cell::Cell::new(None), async {
            let trade = f.await?;
            let gross_quote = GROSS.with(std::cell::Cell::get).ok_or_else(|| {
                GatewayError::new(
                    ErrorCode::OutcomeUnknown,
                    Some(trade.request.venue.clone()),
                    "В ответе площадки отсутствует фактический оборот ордера в валюте котировки",
                )
            })?;
            Ok(SpotFill { trade, gross_quote })
        })
        .await
}

/// Итог принятого действия. У вывода это запись отправителя; зачисление подтверждается отдельно.
#[derive(Clone, Serialize, Deserialize)]
pub enum Outcome {
    Order(Box<SpotFill>),
    Withdrawal(Box<Transfer>),
}

/// Сохраняется вместе с реальным идентификатором до первого запроса. Ошибка результата не стирает намерение.
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub operation: Request,
    pub lookup: PendingRef,
    #[serde(default, deserialize_with = "super::settlement_archive::result")]
    pub result: Option<Outcome>,
}

#[derive(Clone)]
struct Action {
    db: Db,
    key: String,
    operation: Request,
}

tokio::task_local! { static ACTION: Action; }

type Check = std::sync::Arc<dyn Fn(&PendingRef) -> GatewayResult<()> + Send + Sync>;
tokio::task_local! { static CHECK: Check; }

/// Поздняя сверка снимка маршрута перед записью первичного идентификатора и внешней отправкой.
pub async fn checked<T>(
    guard: impl Fn(&PendingRef) -> GatewayResult<()> + Send + Sync + 'static,
    f: impl Future<Output = T>,
) -> T {
    CHECK.scope(std::sync::Arc::new(guard), f).await
}
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
                "SELECT body FROM core_state WHERE kind IN (?1,?3) AND id=?2",
                params![KIND, id, LEGACY],
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
    if let Ok(result) = CHECK.try_with(|guard| guard(lookup)) {
        result?;
    }
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
    let original = body.clone();
    let key = id.clone();
    let inserted = db.ledger().write(move |c| {
        c.execute("INSERT INTO core_state(kind,id,body,updated_at) SELECT ?1,?2,?3,?4 WHERE NOT EXISTS(SELECT 1 FROM core_state WHERE kind=?5 AND id=?2) ON CONFLICT(kind,id) DO NOTHING",
            params![KIND,key,body,Utc::now().to_rfc3339(),LEGACY])
    }).await.map_err(|e| error(e.to_string()))?;
    if inserted == 1 {
        if let Ok(Err(reason)) = CHECK.try_with(|guard| guard(lookup)) {
            // Управление ещё у единственного владельца INSERT, POST не начат.
            // Убирается только его собственная нетронутая запись.
            db.ledger()
                .write(move |c| {
                    c.execute(
                        "DELETE FROM core_state WHERE kind=?1 AND id=?2 AND body=?3",
                        params![KIND, id, original],
                    )
                })
                .await
                .map_err(|e| error(e.to_string()))?;
            return Err(reason);
        }
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
    let count=db.ledger().write(move |c| {
        let raw=c.query_row("SELECT body FROM core_state WHERE kind IN (?1,?2) AND id=?3",params![KIND,LEGACY,key],|r|r.get::<_,String>(0)).optional()?;
        let Some(raw)=raw else{return Ok(0)};
        let matches=serde_json::from_str::<Entry>(&raw).ok().and_then(|r|serde_json::to_string(&r).ok()).is_some_and(|s|s==expected);
        if !matches{return Ok(0);}
        c.execute("UPDATE core_state SET body=?1,updated_at=?2 WHERE kind IN (?3,?6) AND id=?4 AND body=?5",
            params![body,Utc::now().to_rfc3339(),KIND,key,raw,LEGACY])
    }).await.map_err(|e|e.to_string())?;
    if count == 1 {
        Ok(())
    } else {
        Err("Шаг Base изменился во время сверки результата".into())
    }
}

/// Сохраняет завершённую без исполнения попытку под её хешем. Сравнение полного прежнего состояния исключает гонку с восстановлением.
pub async fn archive_attempt(
    db: &Db,
    key: &str,
    original: &Entry,
    reason: &str,
) -> Result<(), String> {
    let hash = original
        .lookup
        .venue_ref
        .as_deref()
        .ok_or("Нет идентификатора подготовленной попытки")?;
    let prior = serde_json::to_string(original).map_err(|e| e.to_string())?;
    let after = serde_json::to_string(&serde_json::json!({"attempt":original,"closed":reason}))
        .map_err(|e| e.to_string())?;
    let dst = format!("{key}/closed/{hash}");
    let key = key.to_owned();
    let n = db
        .ledger()
        .write(move |c| {
            c.execute(
        "UPDATE core_state SET id=?1,body=?2,updated_at=?3 WHERE kind IN (?4,?7) AND id=?5 AND body=?6",
        params![dst,after,Utc::now().to_rfc3339(),KIND,key,prior,LEGACY])
        })
        .await
        .map_err(|e| e.to_string())?;
    if n == 1 {
        Ok(())
    } else {
        Err("Подготовленная попытка уже изменилась".into())
    }
}
