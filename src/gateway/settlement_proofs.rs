//! Файл: gateway/settlement_proofs.rs.
//! Денежный круг запоминает проверенные адаптером включения. После перезапуска исполнитель
//! вновь читает исходные операции. Перед следующим внешним действием все включения
//! этого прохода должны оставаться каноническими с настроенным числом подтверждений.
use super::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::core::types::VenueId;
use futures_util::future::BoxFuture;
use std::{cell::RefCell, collections::BTreeMap, future::Future, sync::Arc};

type Check = Arc<dyn Fn() -> BoxFuture<'static, GatewayResult<()>> + Send + Sync>;
type Proofs = BTreeMap<(String, String), (String, Check)>;
tokio::task_local! { static PROOFS:RefCell<Proofs>; }

pub async fn scope<T>(work: impl Future<Output = T>) -> T {
    PROOFS.scope(RefCell::default(), work).await
}
pub fn active() -> bool {
    PROOFS.try_with(|_| ()).is_ok()
}

pub fn record(
    venue: &VenueId,
    tx: &str,
    block: String,
    verify: impl Fn() -> BoxFuture<'static, GatewayResult<()>> + Send + Sync + 'static,
) -> GatewayResult<()> {
    let key = (venue.0.clone(), super::tx_hash::norm(tx));
    PROOFS
        .try_with(|proofs| {
            let mut proofs = proofs.borrow_mut();
            if proofs.get(&key).is_some_and(|(old, _)| old != &block) {
                return Err(GatewayError::new(
                    ErrorCode::OutcomeUnknown,
                    Some(venue.clone()),
                    "Блок исходной операции изменился во время проверки",
                ));
            }
            proofs.insert(key, (block, Arc::new(verify)));
            Ok(())
        })
        .unwrap_or(Ok(()))
}
/// Фиксирует исходное включение до использования денег. Конкурирующая проверка и
/// восстановление могут лишь подтвердить уже записанный хеш блока этой сети.
pub async fn pin(
    db: &crate::storage::db::Db,
    venue: &VenueId,
    chain: u64,
    tx: &str,
) -> Result<(), String> {
    let hash = super::tx_hash::norm(tx);
    let block = PROOFS
        .try_with(|p| {
            p.borrow()
                .get(&(venue.0.clone(), hash.clone()))
                .map(|(block, _)| block.clone())
        })
        .ok()
        .flatten()
        .ok_or_else(|| format!("Нет подтверждённого блока операции {tx} на {}", venue.0))?;
    let key = serde_json::to_string(&(venue, chain, hash)).map_err(|e| e.to_string())?;
    let body = serde_json::to_string(&block).map_err(|e| e.to_string())?;
    let agrees=db.ledger().write(move |c|{
        c.execute("INSERT INTO core_state(kind,id,body,updated_at) VALUES ('settlement_block',?1,?2,?3) ON CONFLICT(kind,id) DO NOTHING",rusqlite::params![key,body,chrono::Utc::now().to_rfc3339()])?;
        c.query_row("SELECT body=?2 FROM core_state WHERE kind='settlement_block' AND id=?1",rusqlite::params![key,body],|r|r.get::<_,bool>(0))
    }).await.map_err(|e|e.to_string())?;
    if agrees {
        Ok(())
    } else {
        Err(format!(
            "Исходный блок {tx} в сети {chain} заменён. Требуется сверка"
        ))
    }
}

pub(crate) async fn verify() -> GatewayResult<()> {
    let checks = PROOFS
        .try_with(|p| {
            p.borrow()
                .values()
                .map(|(_, f)| f.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    futures_util::future::try_join_all(checks.into_iter().map(|f| f())).await?;
    Ok(())
}
