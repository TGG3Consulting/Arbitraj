//! Файл: storage/cex_funds_original.rs.
//! Назначение: сохраняет идентичность, подготовленный ID и запрос под общим ключом ордера.
//! Потребители: допуск перед POST и холодная сверка конкретного биржевого IOC.
//! Зависимости: та же транзакция ledger.sqlite, что и бронь средств.
//! Настройки: отсутствуют.
use super::{cex_funds::Bucket, ledger_db::LedgerDb};
use crate::{core::types::TradeRequest, gateway::pending::PendingRef};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Original {
    pub account: String,
    pub currency: String,
    pub request: TradeRequest,
    pub lookup: PendingRef,
}
pub fn record(c: &Connection, key: &str, original: &Original) -> rusqlite::Result<()> {
    let body = serde_json::to_string(original)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    c.execute(
        "INSERT INTO cex_funds_original(action,body) VALUES(?1,?2)",
        params![key, body],
    )?;
    Ok(())
}
pub async fn load(db: &LedgerDb, key: &str) -> Result<Option<Original>, String> {
    let key = key.to_string();
    let row = db
        .read(move |c| {
            c.query_row(
                "SELECT body FROM cex_funds_original WHERE action=?1",
                [key],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
        .await
        .map_err(|e| e.to_string())?;
    row.map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| e.to_string())
}
/// Изначальный идентификатор счёта устанавливает адаптер по подписанному ответу площадки.
pub fn binding(o: &Original, b: &Bucket) -> bool {
    o.request.venue.0 == b.venue && o.account == b.account && o.currency == b.currency
}
/// Снимается до HTTP без знания будущего native accountId. Чужие счета не меняют ревизию выбранного.
pub async fn revisions(
    db: &LedgerDb,
    venue: &str,
    currency: &str,
) -> Result<std::collections::HashMap<String, i64>, String> {
    let (venue, currency) = (venue.to_string(), currency.to_string());
    db.read(move|c|{
        let mut q=c.prepare("SELECT json_extract(bucket,'$[1]'),revision FROM cex_funds_epoch WHERE json_extract(bucket,'$[0]')=?1 AND json_extract(bucket,'$[2]')=?2")?;
        q.query_map(params![venue,currency],|r|Ok((r.get(0)?,r.get(1)?)))?.collect()
    }).await.map_err(|e|e.to_string())
}
pub async fn terminal(
    db: &LedgerDb,
    b: &Bucket,
    order: &str,
) -> Result<Option<(Original, rust_decimal::Decimal)>, String> {
    let key =
        serde_json::to_string(&(&b.venue, &b.account, &b.currency)).map_err(|e| e.to_string())?;
    let order = order.to_string();
    let raw=db.read(move|c|c.query_row("SELECT o.body,f.debit FROM cex_funds AS f JOIN cex_funds_original AS o ON o.action=f.action WHERE f.bucket=?1 AND f.order_id=?2",params![key,order],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()).await.map_err(|e|e.to_string());
    let raw = raw?;
    raw.map(|(body, n)| {
        Ok((
            serde_json::from_str(&body).map_err(|e| e.to_string())?,
            n.parse::<rust_decimal::Decimal>()
                .map_err(|e| e.to_string())?,
        ))
    })
    .transpose()
}

/// Вызывается только победителем admit, если последняя проверка отказала до начала HTTP POST.
pub async fn abort(
    db: &LedgerDb,
    b: &Bucket,
    action: &str,
    client: &str,
    marker: Option<(String, String)>,
) -> Result<(), String> {
    let bucket =
        serde_json::to_string(&(&b.venue, &b.account, &b.currency)).map_err(|e| e.to_string())?;
    let (action, client) = (action.to_string(), client.to_string());
    let watch = db.cash_watch.clone();
    let watch_venue = b.venue.clone();
    db.write(move|c|{
        let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let ok=tx.query_row("SELECT EXISTS(SELECT 1 FROM cex_funds WHERE action=?1 AND bucket=?2 AND client=?3 AND debit IS NULL)",params![action,bucket,client],|r|r.get::<_,bool>(0))?;
        if !ok{return Err(rusqlite::Error::QueryReturnedNoRows)}
        if let Some((key,body))=marker && !crate::gateway::settlement_intent::remove_cash_marker(&tx,&key,&body)?{return Err(rusqlite::Error::QueryReturnedNoRows)}
        tx.execute("DELETE FROM cex_funds_original WHERE action=?1",[&action])?;
        tx.execute("DELETE FROM cex_funds WHERE action=?1",[&action])?;
        tx.execute("UPDATE cex_funds_epoch SET revision=revision+1 WHERE bucket=?1",[bucket])?;
        tx.commit()?;
        watch.changed(&watch_venue,&action,None);
        Ok(())
    }).await.map_err(|e|e.to_string())
}

/// Этот квиток доказывает наблюдение совершённого списания. После закрытия всех mixed-поручений
/// его история больше не будит обычный маршрут. Сам дебет хранится и при следующем включении
/// проверяется относительно нового баланса (evidence не служит пропуском для другого снимка).
pub(crate) async fn observed(
    db: &LedgerDb,
    b: &Bucket,
    order: &str,
    debit: rust_decimal::Decimal,
    evidence: serde_json::Value,
) -> Result<(), String> {
    let bucket =
        serde_json::to_string(&(&b.venue, &b.account, &b.currency)).map_err(|e| e.to_string())?;
    let evidence = serde_json::to_string(&evidence).map_err(|e| e.to_string())?;
    let order = order.to_string();
    let venue = b.venue.clone();
    let watch = db.cash_watch.clone();
    db.write(move |c| {
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (action, net): (String, String) = tx.query_row(
            "SELECT action,debit FROM cex_funds WHERE bucket=?1 AND order_id=?2",
            params![bucket, order],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if net.parse::<rust_decimal::Decimal>().ok() != Some(debit) {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        tx.execute(
            "INSERT OR IGNORE INTO cex_funds_seen(action,evidence) VALUES(?1,?2)",
            params![action, evidence],
        )?;
        tx.commit()?;
        watch.changed(&venue, &action, Some(false));
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())
}
