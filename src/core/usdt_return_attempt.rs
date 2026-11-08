//! Файл: core/usdt_return_attempt.rs
//! Что делает: одна попытка возврата выручки USDT закрытой сделки (решение 163; продолжение usdt_return_retry.rs —
//! размер файла): кошелёк DEX — сделки. Плюс состояние «ждёт повтора» части из учёта, захват попытки (вторая задача по
//! той же части не пройдёт), строка в хронологию и журнал.
//! Решение 227 (живой случай JCT: повтор с суммой «свободные, не больше всей выручки сделки» отправил 387.56 USDT с
//! кошелька w7 — 342.29 выручки этой части и 45.28 оборотных денег другого направления): сумма попытки — РОВНО сумма
//! части (её выручка минус комиссия), никогда не больше; свободно меньше части — попытка не отправляет ничего
//! (ошибка «свободно меньше части», вывод точно не ушёл): дальше повтор или «USDT не вернулись» по решению 163.
//! Допуск (решение 227, допуск; Тигран 08.11): свободно меньше части не больше чем на usdt.return_tolerance_usd —
//! уходит свободный остаток, в хронологию «отправлено X вместо Y …». Это единственная проверка «свободно ≥ части» на
//! пути повтора и «Перегнать» (обе идут через attempt).
//! Кто вызывает: core/usdt_return_retry.rs (повторы), core/manual_usdt.rs («Перегнать»).
//! От чего зависит: usdt_return.rs (relay_try, usdt), limits.rs (free_now), dex_account.rs, ledger.rs, deal.rs.
//! Настройки: venue.quote_asset (через usdt_return.rs), usdt.return_tolerance_usd (площадка, где лежат USDT;
//! необязательная: не задана — строго, как до допуска).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::op_error::OpError;
use super::usdt_return_state::UsdtReturn;

/// Одна попытка возврата части: сумма — ровно сумма части (cap, решение 227), не свободный баланс; при нехватке в
/// пределах допуска — свободный остаток (меньше части, строка в хронологию); кошелёк DEX — сделки. Принимает: ядро,
/// сделку, сумму части, откуда/куда, метку попытки (к ID вывода). Возвращает: (отправлено, зачислено). Ошибки: сделки нет, кошелька нет, баланс не получен, свободно меньше части (чужие USDT не трогаем),
/// перегон не состоялся (OpError).
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
    // Решение 227: никогда не больше части — остальное на площадке чужое (другие части, оборотные деньги кошелька).
    let amount = amount_for(core, from, free, cap)
        .map_err(|e| OpError::from(format!("{} ({}): {e}", from.0, usdt.ticker)))?;
    let id = format!("{deal}{tag}");
    let credited = super::usdt_return::relay_try(core, &id, from, to, acc.as_ref(), amount).await?;
    if amount < cap {
        let t = format!(
            "отправлено {amount} вместо {cap}: на площадке меньше на {} USDT — в пределах допуска usdt.return_tolerance_usd",
            cap - amount
        );
        note(core, deal, &t).await;
    }
    Ok((amount, credited))
}

/// Допуск нехватки возврата части на площадке (usdt.return_tolerance_usd, область — площадка, где лежат USDT).
/// Возвращает: None — не задан (строго). Ошибок нет.
pub fn tolerance(core: &Core, from: &VenueId) -> Option<Decimal> {
    let at = At {
        venue: Some(&from.0),
        coin: None,
    };
    let s = &core.inner.deps.settings;
    s.decimal("usdt.return_tolerance_usd", at).ok()
}

/// Сумма попытки части с допуском площадки (usdt_return_parts::send_amount + tolerance). Принимает: ядро, площадку
/// части, свободно там, сумму части. Возвращает: сумму к отправке (≤ части). Ошибки: как send_amount.
pub fn amount_for(
    core: &Core,
    from: &VenueId,
    free: Decimal,
    cap: Decimal,
) -> Result<Decimal, String> {
    super::usdt_return_parts::send_amount(free, cap, tolerance(core, from))
}

/// Состояние части «ждёт повтора» (решение 227: часть — по площадке): (сумма части, откуда, куда, сделано повторов,
/// причина последнего сбоя). None — сделки или части нет, или состояние другое.
pub(super) fn retrying(
    core: &Core,
    deal: &str,
    at: &VenueId,
) -> Option<(Decimal, VenueId, VenueId, u64, String)> {
    let deals = lk(&core.inner.deals);
    match super::usdt_return_parts::get(deals.get(deal)?, Some(at))?.clone() {
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

/// Захват попытки части: «ждёт повтора» с тем же счётом → «идёт» (в учёте). Возвращает: true — захвачено. Ошибок нет.
pub(super) async fn claim(core: &Core, deal: &str, at: &VenueId, tries: u64) -> bool {
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
        }) = super::usdt_return_parts::get(d, Some(at)).cloned()
        else {
            return false;
        };
        if t != tries {
            return false;
        }
        super::usdt_return_parts::put(d, UsdtReturn::Pending { amount, from, to });
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
