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

/// В прежней обычной покупке отсутствие поля сохраняет исходный ответ площадки.
pub(crate) async fn capture_optional(
    f: impl Future<Output = GatewayResult<TradeResult>>,
) -> (GatewayResult<TradeResult>, Option<rust_decimal::Decimal>) {
    GROSS
        .scope(std::cell::Cell::new(None), async {
            let r = f.await;
            (r, GROSS.with(std::cell::Cell::get))
        })
        .await
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
    #[serde(skip)]
    raw: Option<std::sync::Arc<String>>,
}

impl Entry {
    pub fn prepared(operation: Request, lookup: PendingRef) -> Self {
        Self { operation, lookup, result: None, raw: None }
    }

    fn from_raw(body: String) -> Result<Self, serde_json::Error> {
        let mut entry: Self = serde_json::from_str(&body)?;
        entry.raw = Some(std::sync::Arc::new(body));
        Ok(entry)
    }

    /// CAS сравнивает исходные байты. Изменённый после чтения объект не даёт права на запись.
    fn expected_body(&self) -> Result<std::sync::Arc<String>, String> {
        let rendered = serde_json::to_string(self).map_err(|e| e.to_string())?;
        let Some(raw) = &self.raw else { return Ok(std::sync::Arc::new(rendered)); };
        if raw.as_str() != rendered {
            let original: Self = serde_json::from_str(raw).map_err(|e| e.to_string())?;
            if serde_json::to_string(&original).map_err(|e| e.to_string())? != rendered {
                return Err("Сохранённое поручение изменено после чтения".into());
            }
        }
        Ok(raw.clone())
    }
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

/// Возвращает исходный ключ действия для идентификатора исполнителя внутри той же транзакции.
pub fn current_key() -> Option<String> {
    ACTION.try_with(|action| action.key.clone()).ok()
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
    row.map(Entry::from_raw)
        .transpose()
        .map_err(|e| e.to_string())
}

/// Непосредственно перед внешней отправкой сохраняет реальный prepared ID. Только выигравший INSERT может отправить.
pub async fn before_send(lookup: &PendingRef) -> GatewayResult<()> {
    if let Ok(result) = CHECK.try_with(|guard| guard(lookup)) {
        result?;
    }
    crate::core::cash_order::track_legacy(lookup).await?;
    let action = ACTION.try_with(Clone::clone).ok();
    if crate::core::cash_order::active() {
        super::settlement_proofs::verify().await?;
        let prepared = action
            .map(|a| {
                let row = Entry::prepared(a.operation, lookup.clone());
                serde_json::to_string(&row)
                    .map(|body| (a.key, body))
                    .map_err(|e| error(e.to_string()))
            })
            .transpose()?;
        return crate::core::cash_order::before_send(lookup, prepared).await;
    }
    let Some(action) = action else { return Ok(()) };
    let body = serde_json::to_string(&Entry::prepared(action.operation, lookup.clone()))
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
        let ready = super::settlement_proofs::verify()
            .await
            .and_then(|()| CHECK.try_with(|guard| guard(lookup)).unwrap_or(Ok(())));
        if let Err(reason) = ready {
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

pub(crate) async fn verify_cash_marker(lookup: &PendingRef) -> GatewayResult<()> {
    super::settlement_proofs::verify().await?;
    CHECK.try_with(|guard| guard(lookup)).unwrap_or(Ok(()))
}
pub(crate) fn remove_cash_marker(
    c: &rusqlite::Connection,
    key: &str,
    body: &str,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "DELETE FROM core_state WHERE kind=?1 AND id=?2 AND body=?3",
        params![KIND, key, body],
    )? == 1)
}

/// Смешанный ордер и его денежная бронь пишутся в одной транзакции. Повтор старого
/// имени журнала равносилен существующему действию.
pub(crate) fn insert_cash_marker(
    c: &rusqlite::Connection,
    key: &str,
    body: &str,
) -> rusqlite::Result<()> {
    let wrote=c.execute("INSERT INTO core_state(kind,id,body,updated_at) SELECT ?1,?2,?3,?4 WHERE NOT EXISTS(SELECT 1 FROM core_state WHERE kind=?5 AND id=?2)",
        params![KIND,key,body,Utc::now().to_rfc3339(),LEGACY])?;
    if wrote != 1 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    Ok(())
}

/// Записывает подтверждённый биржей/сетью итог. Сначала проверяет, что сохранён тот же подготовленный шаг.
pub async fn confirm(db: &Db, key: &str, original: &Entry, outcome: Outcome) -> Result<(), String> {
    let expected = original.expected_body()?;
    let result = serde_json::to_string(&outcome).map_err(|e| e.to_string())?;
    let key = key.to_owned();
    let count = db.ledger().write(move |c| c.execute(
        "UPDATE core_state SET body=json_set(body,'$.result',json(?1)),updated_at=?2 WHERE kind IN (?3,?6) AND id=?4 AND body=?5",
        params![result,Utc::now().to_rfc3339(),KIND,key,expected.as_str(),LEGACY],
    )).await.map_err(|e| e.to_string())?;
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
    let prior = original.expected_body()?;
    let reason = reason.to_owned();
    let dst = format!("{key}/closed/{hash}");
    let key = key.to_owned();
    let n = db.ledger().write(move |c| c.execute(
        "UPDATE core_state SET id=?1,body=json_object('attempt',json(body),'closed',?2),updated_at=?3 WHERE kind IN (?4,?7) AND id=?5 AND body=?6",
        params![dst,reason,Utc::now().to_rfc3339(),KIND,key,prior.as_str(),LEGACY],
    )).await.map_err(|e| e.to_string())?;
    if n == 1 {
        Ok(())
    } else {
        Err("Подготовленная попытка уже изменилась".into())
    }
}

/// Сохраняется только односторонний отпечаток публичного идентификатора API.
pub fn account(
    vault: &crate::secrets::Vault,
    name: &str,
) -> crate::gateway::errors::GatewayResult<String> {
    let key = vault.get(name)?;
    if key.trim().is_empty() {
        return Err(crate::gateway::errors::GatewayError::new(
            crate::gateway::errors::ErrorCode::NotConfigured,
            None,
            "Не задан ключ торгового счёта",
        ));
    }
    Ok(super::cash_signer::fingerprint(&key))
}
