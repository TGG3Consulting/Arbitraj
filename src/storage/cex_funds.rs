//! Файл: storage/cex_funds.rs.
//! Назначение: регистрирует максимум списания IOC до отправки и сохраняет точный итог после сверки.
//! Потребители: одновременные биржевые операции в одной валюте счёта, восстановление по client ID.
//! Зависимости: короткие транзакции ledger.sqlite. Сетевые чтения выполняются до транзакции.
//! Настройки: отсутствуют.
use super::ledger_db::LedgerDb;
use rusqlite::{Connection, OptionalExtension, params};
use rust_decimal::Decimal;

#[path = "cex_funds_balance.rs"]
mod balance;
pub use balance::{Balance, Booked, Funding, Holds};

/// Идентичность торгового счёта и фактическая валюта книги балансов.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bucket {
    pub venue: String,
    pub account: String,
    pub currency: String,
}
impl Bucket {
    fn key(&self) -> Result<String, String> {
        if [&self.venue, &self.account, &self.currency]
            .iter()
            .any(|v| v.trim().is_empty())
        {
            return Err("Не установлены биржа, торговый счёт или валюта".into());
        }
        serde_json::to_string(&(&self.venue, &self.account, &self.currency))
            .map_err(|e| e.to_string())
    }
}

/// Один логический этап и его исходный подготовленный идентификатор.
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    pub action: String,
    pub client: String,
    pub deal: String,
    /// Покупка из первоначального бюджета именно этой mixed-сделки.
    pub funding: bool,
    /// Предел вместе с максимальной комиссией в валюте списания.
    pub cap: Decimal,
}

#[derive(Debug, PartialEq)]
pub enum Admission {
    Send,
    Inspect(String),
    Recheck,
    Insufficient,
}
struct Row {
    claim: Claim,
    settled: Option<(String, Decimal)>,
}
fn revision(c: &Connection, key: &str) -> rusqlite::Result<i64> {
    Ok(c.query_row(
        "SELECT revision FROM cex_funds_epoch WHERE bucket=?1",
        [key],
        |r| r.get(0),
    )
    .optional()?
    .unwrap_or(0))
}
fn bump(c: &Connection, key: &str) -> rusqlite::Result<()> {
    c.execute("INSERT INTO cex_funds_epoch(bucket,revision) VALUES(?1,1) ON CONFLICT(bucket) DO UPDATE SET revision=revision+1",[key])?;
    Ok(())
}
/// Вызывается до получения баланса. Новое состояние между чтением и записью требует свежего чтения.
pub async fn epoch(db: &LedgerDb, bucket: &Bucket) -> Result<i64, String> {
    let key = bucket.key()?;
    db.read(move |c| revision(c, &key))
        .await
        .map_err(|e| e.to_string())
}
fn decode(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let parse = |i| -> rusqlite::Result<Decimal> {
        r.get::<_, String>(i)?.parse().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e))
        })
    };
    let order: Option<String> = r.get(6)?;
    let settled = match order {
        Some(id) => Some((id, parse(7)?)),
        None => None,
    };
    Ok(Row {
        claim: Claim {
            action: r.get(0)?,
            client: r.get(2)?,
            deal: r.get(3)?,
            funding: r.get(4)?,
            cap: parse(5)?,
        },
        settled,
    })
}
/// Единственный выигравший INSERT вправе начать отправку. Повтор восстанавливает исходный client ID.
/// Две задачи могут читать биржу параллельно, блокировка SQLite удерживается только для расчёта и записи.
pub async fn admit(
    db: &LedgerDb,
    bucket: &Bucket,
    claim: Claim,
    free: Balance,
    holds: Holds,
) -> Result<Admission, String> {
    let key = bucket.key()?;
    if [&claim.action, &claim.client, &claim.deal]
        .iter()
        .any(|s| s.trim().is_empty())
        || claim.cap <= Decimal::ZERO
        || free.bucket != *bucket
    {
        return Err("Неоднозначные реквизиты резерва или снимка баланса".into());
    }
    db.write(move |c|{
        let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let old=tx.query_row("SELECT bucket,client,deal,funding,cap FROM cex_funds WHERE action=?1",[&claim.action],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?,r.get::<_,String>(4)?))).optional()?;
        if let Some((prior,id,deal,funding,cap))=old{
            return Ok(if prior==key && deal==claim.deal && funding==claim.funding && cap.parse::<Decimal>().ok()==Some(claim.cap){Ok(Admission::Inspect(id))}else{Err("Логический ID уже связан с другими реквизитами".to_string())})
        }
        if revision(&tx,&key)?!=free.epoch{return Ok(Ok(Admission::Recheck))}
        let mut rows={
            let mut stmt=tx.prepare("SELECT action,bucket,client,deal,funding,cap,order_id,debit FROM cex_funds WHERE bucket=?1")?;
            stmt.query_map([&key],decode)?.collect::<Result<Vec<_>,_>>()?
        };
        rows.push(Row{claim:claim.clone(),settled:None});
        match balance::covers(&free,&holds,&rows){
            Err(e)=>return Ok(Err(e)),
            Ok(false)=>return Ok(Ok(Admission::Insufficient)),
            Ok(true)=>{}
        }
        tx.execute("INSERT INTO cex_funds(action,bucket,client,deal,funding,cap) VALUES(?1,?2,?3,?4,?5,?6)",params![claim.action,key,claim.client,claim.deal,claim.funding,claim.cap.to_string()])?;
        bump(&tx,&key)?;
        tx.commit()?;
        Ok(Ok(Admission::Send))
    }).await.map_err(|e|e.to_string())?
}
/// Принимает терминальный итог исходного IOC всех его исполнений с комиссией в валюте списания.
/// Пока комиссия/исход неизвестны, вызывающая сторона сохраняет исходный максимум.
pub async fn finish(
    db: &LedgerDb,
    bucket: &Bucket,
    action: &str,
    client: &str,
    order: &str,
    debit: Decimal,
) -> Result<(), String> {
    let key = bucket.key()?;
    if debit < Decimal::ZERO || order.trim().is_empty() {
        return Err("Неверный итог ордера".into());
    }
    let (action, client, order) = (action.to_string(), client.to_string(), order.to_string());
    db.write(move|c|{
        let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let row=tx.query_row("SELECT action,bucket,client,deal,funding,cap,order_id,debit FROM cex_funds WHERE action=?1 AND bucket=?2 AND client=?3",params![action,key,client],decode).optional()?;
        let Some(row)=row else{return Ok(Err("Не найден исходный резерв счёта и валюты".into()))};
        if let Some((id,old))=row.settled{return Ok(if (id,old)==(order,debit){Ok(())}else{Err("Итог исходного ордера изменился".into())})}
        // Даже если площадка списала больше ожидаемого максимума, учитывается реальный долг.
        tx.execute("UPDATE cex_funds SET order_id=?1,debit=?2 WHERE action=?3",params![order,debit.to_string(),action])?;
        bump(&tx,&key)?;
        tx.commit()?;
        Ok(Ok(()))
    }).await.map_err(|e|e.to_string())?
}
#[cfg(test)]
#[path = "cex_funds_tests.rs"]
mod tests;
