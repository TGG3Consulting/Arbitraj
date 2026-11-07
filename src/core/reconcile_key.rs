//! Файл: core/reconcile_key.rs
//! Что делает: решение 211 (ревью В2) — ключ монеты в сверке балансов (reconcile.rs). У биржи — её тикер монеты (строки
//! balances() биржи — по тикеру). У кошелька DEX — адрес контракта «0x…»: строки balances() DEX подписаны показным
//! именем, а у двух разных монет (разные адреса) показное имя может совпасть — сумма по имени склеила бы их балансы.
//! Остаток монеты по адресу — balance(«0x…») кошелька (контракт Venue::balance, 1–2 запроса к узлу). Нативная монета
//! (BNB, решение 62) остаётся по тикеру: её строка BNB уже несёт WBNB в locked.
//! Кто вызывает: core/reconcile.rs, core/reconcile_confirm.rs (expected).
//! От чего зависит: venue_ticker.rs, ticker_index.rs, convert.rs (GAS_TICKER), gateway/contract.rs.
//! Настройки: нет.

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Asset, Balance, VenueId};
use crate::gateway::contract::{Venue, VenueKind};

use super::engine::{Core, rd};

/// Ключ монеты сделки в учёте площадки: DEX и у монеты есть адрес (не монета газа) — «0x…» (venue_ticker::balance_key);
/// иначе — тикер площадки. Принимает: ядро, монету ядра, площадку. Ошибок нет.
pub fn key(core: &Core, asset: &Asset, venue: &VenueId) -> String {
    if asset.ticker == super::convert::GAS_TICKER {
        return super::venue_ticker::ticker_on(core, asset, venue);
    }
    super::venue_ticker::balance_key(core, asset, venue)
}

/// Имя ключа для человека: адрес — показное имя монеты ядра с этим адресом (нет такой — адрес); тикер — как есть.
/// Ошибок нет.
pub fn name(core: &Core, key: &str) -> String {
    match key.parse::<Address>() {
        Ok(a) => rd(&core.inner.tickers)
            .by_token
            .get(&a)
            .and_then(|v| v.first())
            .map(|x| format!("{} ({key})", x.ticker))
            .unwrap_or_else(|| key.to_string()),
        Err(_) => key.to_string(),
    }
}

/// Сколько монеты на площадке (свободно + заблокировано). DEX и ключ — адрес: остаток по адресу (balance), без суммы
/// строк по имени; иначе — сумма строк balances() с этим тикером. Принимает: вид площадки, шлюз площадки, строки
/// balances(), ключ. Ошибки: текст — запрос остатка по адресу не прошёл.
pub async fn total(
    kind: VenueKind,
    gw: &dyn Venue,
    rows: &[Balance],
    key: &str,
) -> Result<Decimal, String> {
    if kind == VenueKind::Dex && key.parse::<Address>().is_ok() {
        return match gw.balance(key).await {
            Ok(Some(r)) => Ok(r.free + r.locked),
            Ok(None) => Ok(Decimal::ZERO),
            Err(e) => Err(format!("остаток {key} не получен — {}", e.message)),
        };
    }
    Ok(rows
        .iter()
        .filter(|x| x.asset == key)
        .map(|x| x.free + x.locked)
        .sum())
}
