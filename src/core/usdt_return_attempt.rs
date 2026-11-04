//! Файл: core/usdt_return_attempt.rs
//! Что делает: одна попытка возврата выручки USDT закрытой сделки (решение 163; продолжение usdt_return_retry.rs —
//! размер файла): сумма — свободные USDT площадки, не больше выручки сделки; кошелёк DEX — сделки. Плюс состояние
//! «ждёт повтора» из учёта, захват попытки (вторая задача по той же сделке не пройдёт), строка в хронологию и журнал.
//! Кто вызывает: core/usdt_return_retry.rs (повторы), core/manual_usdt.rs («Перегнать»).
//! От чего зависит: usdt_return.rs (relay_try, usdt), limits.rs (free_now), dex_account.rs, ledger.rs, deal.rs.
//! Настройки: venue.quote_asset (через usdt_return.rs).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::op_error::OpError;
use super::usdt_return_state::UsdtReturn;

/// Одна попытка возврата: сумма — свободные USDT площадки, не больше выручки сделки (cap); кошелёк DEX — сделки.
/// Принимает: ядро, сделку, предел, откуда/куда, метку попытки (к ID вывода). Возвращает: (отправлено, зачислено).
/// Ошибки: сделки нет, кошелька нет, баланс не получен или ноль, перегон не состоялся (OpError).
pub async fn attempt(
    core: &Core,
    deal: &str,
    cap: Decimal,
    (from, to): (&VenueId, &VenueId),
    tag: &str,
) -> Result<(Decimal, Decimal), OpError> {
    let d = lk(&core.inner.deals)
        .get(deal)
        .cloned()
        .ok_or_else(|| OpError::from(format!("сделки {deal} нет в учёте")))?;
    let acc = super::dex_account::of(core, &d)?;
    let usdt = super::usdt_return::usdt(core, from)?;
    let free = super::limits::free_now(core, from, acc.as_ref(), &usdt.ticker)
        .await
        .map_err(|e| OpError::from(format!("{}: свободные {}: {e}", from.0, usdt.ticker)))?;
    let amount = free.min(cap);
    if amount <= Decimal::ZERO {
        return Err(OpError::from(format!(
            "на {} нет свободных {}",
            from.0, usdt.ticker
        )));
    }
    let id = format!("{deal}{tag}");
    let credited = super::usdt_return::relay_try(core, &id, from, to, acc.as_ref(), amount).await?;
    Ok((amount, credited))
}

/// Состояние «ждёт повтора»: (выручка, откуда, куда, сделано повторов, причина последнего сбоя). None — сделки нет
/// или состояние другое.
pub(super) fn retrying(
    core: &Core,
    deal: &str,
) -> Option<(Decimal, VenueId, VenueId, u64, String)> {
    match lk(&core.inner.deals).get(deal)?.usdt_return.clone()? {
        UsdtReturn::Retrying {
            amount,
            from,
            to,
            tries,
            why,
        } => Some((amount, from, to, tries, why)),
        _ => None,
    }
}

/// Захват попытки: «ждёт повтора» с тем же счётом → «идёт» (в учёте). Возвращает: true — захвачено. Ошибок нет.
pub(super) async fn claim(core: &Core, deal: &str, tries: u64) -> bool {
    let d = {
        let mut deals = lk(&core.inner.deals);
        let Some(d) = deals.get_mut(deal) else {
            return false;
        };
        let Some(UsdtReturn::Retrying {
            amount,
            from,
            to,
            tries: t,
            ..
        }) = d.usdt_return.clone()
        else {
            return false;
        };
        if t != tries {
            return false;
        }
        d.usdt_return = Some(UsdtReturn::Pending { amount, from, to });
        d.clone()
    };
    super::ledger::save(core, &d).await;
    true
}

/// Строка в хронологию сделки (и сохранить). Ошибок нет: сделки нет — ничего.
pub(super) async fn note(core: &Core, deal: &str, text: &str) {
    let d = {
        let mut deals = lk(&core.inner.deals);
        let Some(d) = deals.get_mut(deal) else {
            return;
        };
        d.note(text.to_string());
        d.clone()
    };
    super::ledger::save(core, &d).await;
    log(core, Level::Warn, &d.at, format!("Сделка {deal}: {text}")).await;
}

/// Запись журнала core.usdt_return. Ошибок нет.
pub(super) async fn log(core: &Core, level: Level, venue: &VenueId, text: String) {
    core.inner
        .deps
        .log(level, "core.usdt_return", Some(&venue.0), text, None)
        .await;
}
