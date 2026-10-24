//! Файл: core/native.rs
//! Что делает: нативная монета сети BSC (BNB) в справочнике (решение 62). Биржа отдаёт BNB в сети BSC без адреса
//! контракта (нативная монета, контракта нет), а на DEX та же монета — токен-обёртка WBNB. По настройке площадки DEX
//! (dex.native_ticker + dex.native_wrapped) запись сети биржи «тикер dex.native_ticker, сеть venue.bsc_network биржи,
//! адрес пуст» получает адрес обёртки — дальше справочник, подписка, предпроверка и сделка работают с ней как с обычной
//! монетой по адресу (пулы WBNB, проба, котировка). Исключение только для нативной монеты: все остальные монеты —
//! строго по адресу контракта у биржи (CLAUDE.md, первичный фильтр).
//! Сопоставление не делается (монета остаётся «без адреса»), если: настройки не заданы; площадки DEX дают разные
//! обёртки или тикеры; у биржи уже есть другая монета с адресом обёртки (две монеты на одном адресе не склеиваются).
//! Кто вызывает: core/registry_sync.rs (справочник сетей биржи), core/precheck.rs (свежий справочник перед сделкой);
//! spendable — core/reconcile.rs (сверка), core/manual_return.rs («Вернуть боту»): BNB на DEX — сверх резерва на газ.
//! От чего зависит: settings, core/types, engine.rs (виды площадок).
//! Настройки: dex.native_ticker, dex.native_wrapped (по площадке DEX); venue.bsc_network (по бирже); risk.min_bnb_balance.

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Asset, CoinNetworkInfo, Side, VenueId};
use crate::gateway::contract::{Venue, VenueKind};
use crate::settings::read::At;
use crate::settings::store::Store;

use super::engine::{Core, rd};

/// Нативная монета и её обёртка на DEX.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Native {
    /// Тикер нативной монеты у биржи (dex.native_ticker), например «BNB».
    pub ticker: String,
    /// Адрес обёртки в BSC (dex.native_wrapped), например WBNB.
    pub wrapped: Address,
}

/// Настройка всех площадок DEX. Принимает: хранилище, площадки DEX. Возвращает: Ok(None) — ни у одной DEX не задана
/// пара dex.native_ticker + dex.native_wrapped; Err — площадки DEX расходятся (тогда сопоставления нет). Ошибок узла нет.
pub fn read(s: &Store, dex: &[VenueId]) -> Result<Option<Native>, String> {
    let mut found: Vec<(VenueId, Native)> = vec![];
    for v in dex {
        let at = At {
            venue: Some(&v.0),
            coin: None,
        };
        if let (Ok(ticker), Ok(wrapped)) = (
            s.text("dex.native_ticker", at),
            s.address("dex.native_wrapped", at),
        ) {
            found.push((v.clone(), Native { ticker, wrapped }));
        }
    }
    match found.first() {
        None => Ok(None),
        Some((_, n)) if found.iter().all(|(_, x)| x == n) => Ok(Some(n.clone())),
        Some(_) => Err(format!(
            "площадки DEX дают разную нативную монету: {found:?}"
        )),
    }
}

/// Сопоставляет запись нативной монеты биржи с обёрткой. Принимает: справочник сетей биржи, название её сети BSC
/// (venue.bsc_network; None — не задано), нативную монету. Возвращает: тот же список, где у записи «тикер нативной,
/// сеть BSC, адреса нет» стоит адрес обёртки. Список не меняется, если сеть BSC не задана или у биржи уже есть монета
/// с адресом обёртки. Ошибок нет.
pub fn apply(
    mut list: Vec<CoinNetworkInfo>,
    bsc: Option<&str>,
    n: &Native,
) -> Vec<CoinNetworkInfo> {
    let Some(bsc) = bsc else { return list };
    if list.iter().any(|i| i.asset.bsc_contract == Some(n.wrapped)) {
        return list;
    }
    for i in list.iter_mut().filter(|i| {
        i.asset.ticker == n.ticker && i.network == bsc && i.asset.bsc_contract.is_none()
    }) {
        i.asset.bsc_contract = Some(n.wrapped);
    }
    list
}

/// Справочник сетей биржи с сопоставлением нативной монеты по текущим настройкам (read + apply).
/// Принимает: ядро, биржу, справочник из её API. Возвращает: список для справочника ядра. Ошибок нет: настройки не
/// заданы или расходятся — список как есть (нативная монета остаётся без адреса, причина видна на экране «Монеты»).
pub fn map(core: &Core, venue: &VenueId, list: Vec<CoinNetworkInfo>) -> Vec<CoinNetworkInfo> {
    let s = &core.inner.deps.settings;
    let dex: Vec<VenueId> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
        .map(|(v, _)| v.clone())
        .collect();
    let Ok(Some(n)) = read(s, &dex) else {
        return list;
    };
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    let bsc = s.text("venue.bsc_network", at).ok();
    apply(list, bsc.as_deref(), &n)
}

/// BNB на DEX: из количества вычитается газ продажи (BNB-затраты котировки продажи на это количество: газ обмена,
/// обёртки, чаевые) — продажа платит его из резерва, и сделка «Вернуть боту» не должна в него упереться. Остальные
/// монеты и площадки — без изменений. Принимает: ядро, шлюз площадки, площадку, монету, количество.
/// Возвращает: количество и заметку для хронологии (котировки нет — газ не вычтен; отказ резерва при продаже тогда
/// уводит сделку в «застрявшую монету», core/sell.rs). Ошибок нет.
pub async fn minus_sell_gas(
    core: &Core,
    gw: &std::sync::Arc<dyn Venue>,
    venue: &VenueId,
    asset: &Asset,
    qty: Decimal,
) -> (Decimal, Option<String>) {
    if asset.ticker != super::convert::GAS_TICKER
        || super::trade_ops::kind(core, venue).ok() != Some(VenueKind::Dex)
    {
        return (qty, None);
    }
    match gw.quote(asset, Side::Sell, qty).await {
        Ok(q) => {
            let gas: Decimal = q
                .costs
                .iter()
                .filter(|c| super::convert::is_gas(c.kind) && c.asset == super::convert::GAS_TICKER)
                .map(|c| c.amount)
                .sum();
            (
                qty - gas,
                Some(format!("BNB: за вычетом газа продажи {gas}")),
            )
        }
        Err(e) => (
            qty,
            Some(format!(
                "BNB: газ продажи не оценён ({}) — не вычтен",
                e.message
            )),
        ),
    }
}

/// Тикер нативной монеты по текущим настройкам площадок DEX (read). None — не задано или расходится. Ошибок нет.
pub fn ticker(core: &Core) -> Option<String> {
    let dex: Vec<VenueId> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
        .map(|(v, _)| v.clone())
        .collect();
    read(&core.inner.deps.settings, &dex)
        .ok()
        .flatten()
        .map(|n| n.ticker)
}

/// Меняет ли настройка сопоставление (тогда справочник сетей бирж перечитывается сразу). Ошибок нет.
pub fn affects(name: &str) -> bool {
    name.starts_with("dex.native_") || name == "venue.bsc_network"
}

/// Сколько монет сделки на площадке можно считать своими: у BNB на DEX (монета газа) — баланс минус резерв на газ
/// risk.min_bnb_balance (решение 62: резерв сделкой не тратится); у остальных — баланс как есть. Принимает: ядро,
/// площадку, тикер, баланс. Ошибки: текст — резерв не задан (сверить или вернуть BNB нельзя), площадки нет в реестре.
pub fn spendable(
    core: &Core,
    venue: &VenueId,
    ticker: &str,
    balance: Decimal,
) -> Result<Decimal, String> {
    if ticker != super::convert::GAS_TICKER
        || super::trade_ops::kind(core, venue)? != VenueKind::Dex
    {
        return Ok(balance);
    }
    let reserve = core
        .inner
        .deps
        .settings
        .decimal("risk.min_bnb_balance", At::default())
        .map_err(|e| {
            format!(
                "{}: {ticker} без резерва на газ не посчитать — {}",
                venue.0, e.message
            )
        })?;
    Ok(balance - reserve)
}
