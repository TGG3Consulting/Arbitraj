//! Файл: gateway/settlement_archive.rs.
//! Что делает: читает архив намерений при остановленной или переназначенной площадке.
//! Кто вызывает: интерфейс сверки и десериализация старых записей.
//! От чего зависит: ledger.sqlite и формат результата адаптера.
//! Настройки: не требуются; сетевых запросов нет.
use super::settlement_intent::{KIND, LEGACY, Outcome};
use crate::core::types::TradeResult;
use serde::{Deserialize, Deserializer};

/// Первые записи содержали TradeResult без точной суммы валюты котировки. Возвращаем сохранённый
/// идентификатор на повторную сверку, сохраняя исходную запись до получения точного исполнения.
pub(super) fn result<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Outcome>, D::Error> {
    use serde::de::Error;
    let Some(value) = Option::<serde_json::Value>::deserialize(d)? else {
        return Ok(None);
    };
    match serde_json::from_value::<Outcome>(value.clone()) {
        Ok(done) => Ok(Some(done)),
        Err(_)
            if value
                .get("Order")
                .is_some_and(|v| serde_json::from_value::<TradeResult>(v.clone()).is_ok()) =>
        {
            Ok(None)
        }
        Err(e) => Err(D::Error::custom(e)),
    }
}

/// Локальные факты и ID всех денежных шагов сделки независимо от текущей конфигурации адаптеров.
pub async fn deal(
    db: &crate::storage::db::Db,
    deal: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let deal = deal.to_owned();
    let raw=db.ledger().read(move|c|{
        let mut stmt=c.prepare("SELECT body FROM core_state WHERE kind IN (?1,?2) AND
        (json_extract(body,'$.operation.Order.deal_id')=?3 OR json_extract(body,'$.operation.Withdrawal.deal_id')=?3)")?;
        stmt.query_map(rusqlite::params![KIND,LEGACY,deal],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()
    }).await.map_err(|e|e.to_string())?;
    raw.into_iter()
        .map(|v| serde_json::from_str(&v).map_err(|e| e.to_string()))
        .collect()
}
