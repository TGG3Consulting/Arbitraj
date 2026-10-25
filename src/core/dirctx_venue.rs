//! Файл: core/dirctx_venue.rs
//! Что делает: данные биржи для контекста направления (решение 66): правила спот-пары (шаг, пределы ордера) из
//! справочника пар, сеть котируемой монеты B из справочника сетей и свежесть справочника сетей — комиссии вывода и
//! возврата расчёт берёт из него без запросов к бирже, поэтому справочник старше venue.networks_stale_s (по площадке)
//! — сделки нет; настройка не задана — сделки нет.
//! Здесь же — проверка минимального депозита B (DirCtx::check_deposit, решение 35).
//! Кто вызывает: core/dirctx.rs, core/costs.rs.
//! От чего зависит: engine.rs (рынок, справочник сетей, виды площадок), settings.
//! Настройки: venue.networks_stale_s [площадка].

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{CoinNetworkInfo, VenueId};
use crate::gateway::contract::VenueKind;
use crate::gateway::events::SpotPairState;
use crate::settings::read::At;

use super::engine::{Core, rd};
use super::size::Fail;

/// Правила спот-пары монеты на бирже — из последнего справочника пар (MarketEvent::SpotPairs). У DEX — None.
/// Принимает: ядро, площадку, тикер. Ошибки: у биржи пары нет или шаг количества не больше нуля — сделки нет.
pub fn pair_rules(core: &Core, v: &VenueId, ticker: &str) -> Result<Option<SpotPairState>, String> {
    let cex = rd(&core.inner.kinds)
        .iter()
        .any(|(id, k)| id == v && *k == VenueKind::Cex);
    if !cex {
        return Ok(None);
    }
    match rd(&core.inner.market).pair(v, ticker) {
        Some(p) if p.step_size > Decimal::ZERO => Ok(Some(p.clone())),
        Some(p) => Err(format!(
            "{}: шаг количества пары {ticker} = {}",
            v.0, p.step_size
        )),
        None => Err(format!(
            "{}: пары {ticker} нет в справочнике пар — шаг и минимум ордера неизвестны",
            v.0
        )),
    }
}

/// Сеть монеты у площадки из справочника сетей ядра. Принимает: площадку, тикер, сеть. None — нет. Ошибок нет.
pub fn network(core: &Core, v: &VenueId, ticker: &str, net: &str) -> Option<CoinNetworkInfo> {
    rd(&core.inner.networks).get(v).and_then(|all| {
        all.iter()
            .find(|n| n.asset.ticker == ticker && n.network == net)
            .cloned()
    })
}

/// Справочник сетей не старше venue.networks_stale_s площадки. Принимает: сеть монеты из справочника.
/// Ошибки: настройка не задана; справочник старше — текст с возрастом (виден в «Кандидатах»).
pub fn fresh(core: &Core, i: &CoinNetworkInfo) -> Result<(), String> {
    let at = At {
        venue: Some(&i.venue.0),
        coin: None,
    };
    let max = core
        .inner
        .deps
        .settings
        .integer("venue.networks_stale_s", at)
        .map_err(|e| e.message)?;
    let age = (Utc::now() - i.updated_at).num_seconds();
    if age > max as i64 {
        return Err(format!(
            "{}: справочник сетей ({} {}) старше venue.networks_stale_s: {age} с > {max} с — комиссия вывода не проверена",
            i.venue.0, i.asset.ticker, i.network
        ));
    }
    Ok(())
}

impl super::dirctx::DirCtx {
    /// Минимальный депозит B (решение 35). У DEX ограничений нет.
    /// Ошибки: Stop — минимум неизвестен; TooSmall — приход меньше минимума.
    pub fn check_deposit(&self, qty: Decimal) -> Result<(), Fail> {
        let Some(i) = &self.b.info else { return Ok(()) };
        match i.deposit_min {
            None => Err(Fail::Stop(format!(
                "{}: минимальный депозит неизвестен — сделки нет",
                self.dir.sell_on.0
            ))),
            Some(min) if qty < min => Err(Fail::TooSmall(format!(
                "приход {qty} меньше минимального депозита {min}"
            ))),
            Some(_) => Ok(()),
        }
    }
}
