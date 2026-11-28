//! Файл: core/cash_same.rs
//! Что делает: равные расчётные валюты сторон (найдено арбитром в живом боте 28.11.2026, сборка e9907a2f). У DEX с
//! dex.settlement_contract и биржи одинаковый venue.quote_asset = USDT (evm_profiles::LEDGER_QUOTE) — расчёт прямой, без
//! денежного круга конверсии; закреплённый ERC-20 DEX должен совпасть с адресом USDT сети, который сообщили биржи
//! (registry_chains::usdt_of). Не совпал или не подтверждён — явная причина «адрес USDT сети не подтверждён» вместо
//! вечного NoPrice «Расчётные валюты сторон совпадают» (Market::new) без замеров цен. Денежный круг при равных именах
//! остаётся только там, где биржа зовёт закреплённый ERC-20 иначе (Market::new проходит, cash_alias) —
//! settlement_market.rs mixed.
//! Кто вызывает: core/settlement_market.rs (mixed), core/prices.rs (сигнал), core/profit.rs (вход).
//! От чего зависит: settlement_market.rs (ticker, configured), registry_chains.rs (usdt_of), settings/evm_profiles.rs.
//! Настройки: venue.quote_asset, dex.settlement_contract, dex.chain — только чтение, новых нет.

use crate::core::types::{Direction, VenueId};
use crate::settings::evm_profiles::{self, LEDGER_QUOTE};

use super::engine::Core;
use super::settlement_market::{configured, ticker};

/// Равные имена: у `dex` задан dex.settlement_contract, и venue.quote_asset у `dex` и `cex` — оба USDT.
/// Принимает: ядро, площадку с контрактом расчёта, площадку-напарника. Возвращает: да/нет. Ошибок нет (не прочитано —
/// нет).
pub(super) fn same_labels(core: &Core, dex: &VenueId, cex: &VenueId) -> bool {
    if !evm_profiles::uses_settlement_contract(&core.inner.deps.settings, &dex.0) {
        return false;
    }
    matches!(
        (ticker(core, dex), ticker(core, cex)),
        (Ok(a), Ok(b)) if a == b && b == LEDGER_QUOTE
    )
}

/// Закреплённый ERC-20 DEX (dex.settlement_contract) — тот же, что адрес USDT сети по биржам.
/// Принимает: ядро, DEX. Возвращает: Ok — совпал. Ошибки: текст «адрес USDT сети не подтверждён: …» с причиной.
pub(super) fn confirmed(core: &Core, dex: &VenueId) -> Result<(), String> {
    let why = |e: String| format!("адрес USDT сети не подтверждён: {e}");
    let pinned = configured(core, dex).map_err(why)?.ok_or_else(|| {
        why(format!(
            "{}: не задан dex.settlement_contract для {LEDGER_QUOTE}",
            dex.0
        ))
    })?;
    let Some(contract) = pinned.contract else {
        return Err(why(format!("{}: нет адреса расчётного токена", dex.0)));
    };
    match super::registry_chains::usdt_of(core, pinned.chain.as_ref()) {
        Ok(usdt) if usdt == contract => Ok(()),
        Ok(usdt) => Err(why(format!(
            "{}: dex.settlement_contract {contract} ≠ USDT сети по биржам {usdt}",
            dex.0
        ))),
        Err(e) => Err(why(e)),
    }
}

/// Проверка направления без денежного круга (settlement_market::mixed = нет): у каждой пары DEX ↔ биржа с равными
/// именами USDT адрес USDT сети подтверждён. Принимает: ядро, направление. Ошибки: текст confirmed.
pub(super) fn check(core: &Core, d: &Direction) -> Result<(), String> {
    for (dex, cex) in [(&d.buy_on, &d.sell_on), (&d.sell_on, &d.buy_on)] {
        if same_labels(core, dex, cex) {
            confirmed(core, dex)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "cash_same_tests.rs"]
mod tests;
