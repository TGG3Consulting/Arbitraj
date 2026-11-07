//! Файл: core/transfer_tax.rs
//! Что делает: налог перевода монеты при выводе с биржи (решение 118, Я-3). Источник — проверка налогов любой
//! DEX-площадки по адресу контракта (market.coin_check, TokenTaxes.transfer_pct; несколько DEX — наибольший).
//! Неизвестен: получатель — DEX — ошибка (проверка там обязательна, монета без неё в чёрном списке); биржа ↔ биржа —
//! по настройке cex_cex.untaxed_ok («да» — 0, «нет» или не задана — направления нет). Нативная монета — 0.
//! Кто вызывает: core/dirctx.rs (налог вывода с A в расчёте), credit_expect.rs (ожидаемый приход),
//! rebuild_blacklist.rs (направления биржа ↔ биржа).
//! От чего зависит: market_state.rs (проверки налогов), native.rs, engine.rs, settings.
//! Настройки: cex_cex.untaxed_ok (вся система, необязательная).

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Asset, CheckState, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::engine::{Core, rd};
use super::market_state::MarketState;

/// Текст причины «налог не измерен» для биржа ↔ биржа (виден на «Монетах» и в отказе расчёта).
pub const UNMEASURED: &str = "налог перевода не измерен (ни одна DEX не проверила монету) — биржа ↔ биржа \
                              не торгуется, пока cex_cex.untaxed_ok не «да»";

/// Измеренный налог перевода токена, %: наибольший из проверок налогов всех DEX-площадок. None — не измерен.
/// Принимает: рынок, площадки с видом, адрес токена. Ошибок нет.
pub fn measured(
    market: &MarketState,
    kinds: &[(VenueId, VenueKind)],
    token: Address,
) -> Option<Decimal> {
    kinds
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
        .filter_map(
            |(v, _)| match market.coin_check(v, token).map(|c| &c.taxes) {
                Some(CheckState::Known(t)) => Some(t.transfer_pct),
                _ => None,
            },
        )
        .max()
}

/// Можно ли биржа ↔ биржа без измеренного налога (cex_cex.untaxed_ok = да). Не задана — нет. Ошибок нет.
pub fn untaxed_ok(s: &Store) -> bool {
    s.flag("cex_cex.untaxed_ok", At::default()).unwrap_or(false)
}

/// Налог перевода при выводе с биржи на площадку `to`, %. Принимает: ядро, получателя, монету.
/// Возвращает: процент (0 — нативная монета или биржа ↔ биржа с cex_cex.untaxed_ok = да).
/// Ошибки: текст — нет адреса контракта; налог не измерен (получатель DEX — проверка не закончена; биржа ↔ биржа —
/// cex_cex.untaxed_ok не «да»).
pub fn pct(core: &Core, to: &VenueId, asset: &Asset) -> Result<Decimal, String> {
    // Решение 211 (ревью К1): нативная — по тикеру И адресу обёртки (native::is_native), не любой токен с этим тикером.
    if super::native::is_native(core, asset) {
        return Ok(Decimal::ZERO);
    }
    let token = asset.bsc_contract.ok_or_else(|| {
        format!(
            "{}: нет адреса контракта — налог перевода неизвестен",
            asset.ticker
        )
    })?;
    let kinds = rd(&core.inner.kinds).clone();
    if let Some(p) = measured(&rd(&core.inner.market), &kinds, token) {
        return Ok(p);
    }
    let to_dex = kinds.iter().any(|(v, k)| v == to && *k == VenueKind::Dex);
    if to_dex {
        return Err(format!("{}: налог перевода не проверен", asset.ticker));
    }
    if untaxed_ok(&core.inner.deps.settings) {
        return Ok(Decimal::ZERO);
    }
    Err(format!("{}: {UNMEASURED}", asset.ticker))
}
