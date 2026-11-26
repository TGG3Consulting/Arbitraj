//! Файл: core/cash_holds.rs.
//! Назначение: читает незаконченные денежные обязательства из того же среза, в котором создаётся бронь.
//! Потребители: переданная в cex_funds::admit функция read_holds.
//! Зависимости: индекс текущих состояний deal в ledger.sqlite и штатный расчёт подтверждённых возвратов.
//! Настройки: исходная валюта бюджета сохранена в плане сделки.
use super::{deal::Deal, types::VenueId};
use crate::storage::cex_funds::{Bucket, Funding, HOLD_FILTER, Holds};
use rusqlite::Connection;
use rust_decimal::Decimal;

/// Старые резервы возвратов принадлежат учтённой площадке. До появления native accountId
/// в старых сделках перенос реквизитов не делает эти обязательства свободными.
pub fn read(c: &Connection, bucket: &Bucket) -> Result<Holds, String> {
    read_for(c, bucket, None, None)
}
pub(super) fn read_for(
    c: &Connection,
    bucket: &Bucket,
    own: Option<&str>,
    signer: Option<&str>,
) -> Result<Holds, String> {
    let mut q = c
        .prepare_cached(&format!(
            "SELECT body FROM core_state INDEXED BY cex_funds_held_deals WHERE {HOLD_FILTER}"
        ))
        .map_err(|e| e.to_string())?;
    let mut rows = q.query([]).map_err(|e| e.to_string())?;
    let mut holds = Holds::default();
    let venue = VenueId(bucket.venue.clone());
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let json: String = row.get(0).map_err(|e| e.to_string())?;
        let d: Deal = serde_json::from_str(&json)
            .map_err(|e| format!("Не удаётся подтвердить резерв сохранённой сделки: {e}"))?;
        if d.direction.buy_on == venue
            && own != Some(d.id.as_str())
            && d.buy.is_none()
            && matches!(
                d.step,
                super::deal::Step::Buying | super::deal::Step::Halted
            )
            && d.plan.get(super::settlement_cycle::FIELD).is_none()
            && d.plan.pointer("/size/budget").is_some()
        {
            let row = d
                .plan
                .get(super::cash_order::OPENING)
                .cloned()
                .and_then(|v| {
                    serde_json::from_value::<crate::storage::cex_funds_watch::Opening>(v).ok()
                });
            let Some(row) = row else {
                return Err(format!(
                    "У покупки {} неизвестна исходная валюта или принадлежность счёту",
                    d.id
                ));
            };
            if row.currency == bucket.currency {
                if Some(row.owner.as_str()) != signer || row.cap <= Decimal::ZERO {
                    return Err(format!(
                        "Не подтверждены владелец и верхний предел покупки {}",
                        d.id
                    ));
                }
                let key =
                    serde_json::to_string(&(&bucket.venue, &bucket.account, &bucket.currency))
                        .map_err(|e| e.to_string())?;
                let named=c.query_row("SELECT EXISTS(SELECT 1 FROM cex_funds WHERE bucket=?1 AND deal=?2 AND (debit IS NULL OR CAST(debit AS REAL)>0))",rusqlite::params![key,d.id],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())?;
                if !named {
                    holds.other = holds
                        .other
                        .checked_add(row.cap)
                        .ok_or("Сумма прежних покупок вне диапазона")?;
                }
            }
        }
        if bucket.currency == crate::settings::evm_profiles::LEDGER_QUOTE {
            holds.other = holds
                .other
                .checked_add(super::usdt_hold::saved_returns(&d, &venue)?)
                .ok_or("Сумма обязательств возврата вне диапазона")?;
        }
        if d.is_open()
            && let Some(p) = d.plan.get(super::settlement_cycle::FIELD)
            && p.pointer("/market/cex").and_then(|v| v.as_str()) == Some(&bucket.venue)
            && p.pointer("/market/quote").and_then(|v| v.as_str()) == Some(&bucket.currency)
        {
            let cap = p
                .get("quote_cap")
                .cloned()
                .and_then(|v| serde_json::from_value::<Decimal>(v).ok())
                .ok_or("Исходный бюджет открытой сделки не прочитан")?;
            holds.funding.push(Funding {
                deal: d.id,
                cap,
                invested: d.invested,
            });
        }
    }
    Ok(holds)
}
