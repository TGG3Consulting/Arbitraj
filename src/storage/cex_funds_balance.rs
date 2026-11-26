//! Файл: storage/cex_funds_balance.rs.
//! Назначение: считает покрытие всех обещаний в одной валюте одной книги балансов.
//! Потребители: атомарное решение admit в cex_funds.rs.
//! Зависимости: суммы терминальных исполнений и проверенные записи выбранного биржевого снимка.
//! Настройки: отсутствуют.
use super::{Bucket, Row};
use rust_decimal::Decimal;
use std::collections::HashMap;

/// Учетная запись, доказанно включённая в конкретный свободный баланс.
/// Проводка относится к тому же физическому счету и валюте. Сумма покрывает весь дебет с комиссией.
#[derive(Clone)]
pub struct Booked {
    pub bucket: Bucket,
    pub order: String,
    pub debit: Decimal,
}
pub struct Balance {
    pub bucket: Bucket,
    pub free: Decimal,
    pub epoch: i64,
    pub booked: Vec<Booked>,
}
/// Неиспользованная часть выделенного при открытии mixed-сделки бюджета.
pub struct Funding {
    pub deal: String,
    pub cap: Decimal,
    pub invested: Decimal,
}
#[derive(Default)]
pub struct Holds {
    /// Возвраты, выводы и прочие обязательства, кроме первоначальных бюджетов ниже.
    pub other: Decimal,
    pub funding: Vec<Funding>,
}
fn add(into: &mut Decimal, n: Decimal) -> Result<(), String> {
    *into = into.checked_add(n).ok_or("Сумма резервов вне диапазона")?;
    Ok(())
}
#[derive(Default)]
struct Used {
    pending: Decimal,
    terminal: Decimal,
}
pub(super) fn covers(free: &Balance, holds: &Holds, rows: &[Row]) -> Result<bool, String> {
    if free.free < Decimal::ZERO || holds.other < Decimal::ZERO {
        return Err("Отрицательный баланс или резерв".into());
    }
    let mut booked = HashMap::new();
    for record in &free.booked {
        if record.bucket != free.bucket
            || record.debit < Decimal::ZERO
            || record.order.trim().is_empty()
            || booked.insert(&record.order, record.debit).is_some()
        {
            return Err("Неоднозначная проводка или чужая книга баланса".into());
        }
    }
    let mut total = holds.other;
    let mut used = HashMap::<&str, Used>::new();
    for row in rows {
        let claim = &row.claim;
        let cost = match &row.settled {
            Some((id, debit)) => {
                if claim.funding {
                    add(&mut used.entry(&claim.deal).or_default().terminal, *debit)?
                }
                match booked.get(id) {
                    Some(amount) if amount == debit => Decimal::ZERO,
                    Some(_) => {
                        return Err("Дебет в балансовой проводке отличается от итога ордера".into());
                    }
                    None => *debit,
                }
            }
            None => {
                if claim.funding {
                    add(&mut used.entry(&claim.deal).or_default().pending, claim.cap)?
                }
                claim.cap
            }
        };
        add(&mut total, cost)?;
    }
    let mut seen = std::collections::HashSet::new();
    for h in &holds.funding {
        if h.deal.trim().is_empty()
            || h.cap < Decimal::ZERO
            || h.invested < Decimal::ZERO
            || !seen.insert(&h.deal)
        {
            return Err("Неоднозначный исходный бюджет сделки".into());
        }
        let used = used.remove(h.deal.as_str()).unwrap_or_default();
        let unused = (h.cap - h.invested.max(used.terminal)).max(Decimal::ZERO);
        add(&mut total, (unused - used.pending).max(Decimal::ZERO))?;
    }
    Ok(total <= free.free)
}
