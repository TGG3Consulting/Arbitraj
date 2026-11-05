//! Файл: core/dex_account.rs
//! Что делает: счёт (кошелёк) DEX направления и сделки (решение 132). Направление биржа ↔ DEX — строка маппинга этой
//! биржи; DEX ↔ DEX — строка «DEX ↔ DEX»; биржа ↔ биржа — без счёта. Нет строки — отказ с причиной NO_WALLET (биржа без
//! кошелька с DEX не торгует, циклы не исполняются). Сделка запоминает счёт при открытии (deal.dex_account); все её
//! шаги берут площадку по счёту СДЕЛКИ (venue_deal), а не по текущему маппингу. Сделки до решения 132 (без счёта) —
//! кошелёк переноса. Ядро знает только id счёта: адреса и ключи — у адаптера (шлюз: Registry::get_as).
//! Кто вызывает: executor.rs, trade_ops.rs, transfers.rs, limits.rs, precheck*.rs, costs_*.rs, profit.rs, prices.rs,
//! manual_halted.rs, cycle_plan.rs, cycle_deal.rs, reconcile.rs; api/wallets.rs (сделки кошелька).
//! От чего зависит: deps.rs, engine.rs, gateway/registry.rs, settings/accounts/ (маппинг).
//! Настройки: нет (маппинг — экран «Маппинг кошельков»).

use std::sync::Arc;

use crate::core::types::{DexAccountId, Direction, VenueId};
use crate::gateway::contract::{Venue, VenueKind};
use crate::settings::accounts::ROW_DEX_DEX;

use super::deal::Deal;
use super::deps::Deps;
use super::engine::{Core, lk, rd};

/// Причина отказа направления без кошелька в маппинге.
pub const NO_WALLET: &str = "нет кошелька DEX в маппинге (экран «Маппинг кошельков»)";

/// Строка маппинга направления по видам площадок. Принимает: виды площадок покупки и продажи, их id.
/// Возвращает: id строки (id биржи или ROW_DEX_DEX) или None — DEX не участвует. Ошибок нет.
pub fn row(buy: (&VenueId, VenueKind), sell: (&VenueId, VenueKind)) -> Option<String> {
    match (buy.1, sell.1) {
        (VenueKind::Dex, VenueKind::Dex) => Some(ROW_DEX_DEX.to_string()),
        (VenueKind::Dex, VenueKind::Cex) => Some(sell.0.0.clone()),
        (VenueKind::Cex, VenueKind::Dex) => Some(buy.0.0.clone()),
        (VenueKind::Cex, VenueKind::Cex) => None,
    }
}

/// Счёт направления по текущему маппингу. Принимает: зависимости ядра, направление. Возвращает: None — DEX не
/// участвует. Ошибки: NO_WALLET (строки нет), площадка не подключена.
pub async fn of_direction(deps: &Deps, d: &Direction) -> Result<Option<DexAccountId>, String> {
    let kb = deps.gw.get(&d.buy_on).await.map_err(|e| e.message)?.kind();
    let ks = deps.gw.get(&d.sell_on).await.map_err(|e| e.message)?.kind();
    let Some(r) = row((&d.buy_on, kb), (&d.sell_on, ks)) else {
        return Ok(None);
    };
    match deps.settings.accounts().row(&r) {
        Some(id) => Ok(Some(DexAccountId(id))),
        None => Err(format!("{r}: {NO_WALLET}")),
    }
}

/// Площадка для расчёта и предпроверки направления: DEX — на кошелёк строки направления. Принимает: зависимости,
/// направление, площадку. Ошибки: NO_WALLET, площадка не подключена, нет кошелька.
pub async fn venue_dir(deps: &Deps, d: &Direction, v: &VenueId) -> Result<Arc<dyn Venue>, String> {
    let acc = of_direction(deps, d).await?;
    deps.gw.get_as(v, acc.as_ref()).await.map_err(|e| e.message)
}

/// Счёт сделки: записанный при открытии; у сделки до решения 132 с DEX — кошелёк переноса. Принимает: ядро, id
/// сделки. Возвращает: None — DEX в сделке не участвует или сделки нет в учёте. Ошибки: сделка с DEX без счёта и
/// без кошелька переноса.
pub fn of_deal(core: &Core, deal: &str) -> Result<Option<DexAccountId>, String> {
    let Some(d) = lk(&core.inner.deals).get(deal).cloned() else {
        return Ok(None);
    };
    of(core, &d)
}

/// Счёт сделки по самой сделке (как of_deal, без поиска в учёте). Принимает: ядро, сделку. Ошибки: как у of_deal.
pub fn of(core: &Core, d: &Deal) -> Result<Option<DexAccountId>, String> {
    let deal = &d.id;
    if let Some(a) = &d.dex_account {
        return Ok(Some(a.clone()));
    }
    let kinds = rd(&core.inner.kinds);
    let kind = |v: &VenueId| kinds.iter().find(|(id, _)| id == v).map(|(_, k)| *k);
    let dex = [&d.direction.buy_on, &d.direction.sell_on]
        .iter()
        .any(|v| kind(v) == Some(VenueKind::Dex));
    if !dex {
        return Ok(None);
    }
    match core.inner.deps.settings.accounts().legacy() {
        Some(id) => Ok(Some(DexAccountId(id))),
        None => Err(format!(
            "сделка {deal} начата до маппинга кошельков, а кошелька переноса нет — {NO_WALLET}"
        )),
    }
}

/// Площадка для шага сделки: DEX — на кошелёк СДЕЛКИ. Принимает: ядро, id сделки, площадку.
/// Ошибки: как у of_deal; площадка не подключена; кошелька нет.
pub async fn venue_deal(core: &Core, deal: &str, v: &VenueId) -> Result<Arc<dyn Venue>, String> {
    let acc = of_deal(core, deal)?;
    core.inner
        .deps
        .gw
        .get_as(v, acc.as_ref())
        .await
        .map_err(|e| e.message)
}

/// Площадка для действия по сделке (ручные действия: «Продать сейчас», «Беру на себя»): DEX — кошелёк сделки.
/// Принимает: ядро, сделку, площадку. Ошибки: как у venue_deal.
pub async fn venue_of(core: &Core, d: &Deal, v: &VenueId) -> Result<Arc<dyn Venue>, String> {
    let acc = of(core, d)?;
    core.inner
        .deps
        .gw
        .get_as(v, acc.as_ref())
        .await
        .map_err(|e| e.message)
}

/// Незавершённые сделки кошелька (удалять кошелёк нельзя, пока они есть): все, кроме закрытых и «Беру на себя» —
/// остановленная (Halted) тоже незавершённая: монеты могут лежать на кошельке, ручные действия идут им.
/// Принимает: ядро, id кошелька. Возвращает: id сделок. Ошибок нет.
pub fn open_deals_of(core: &Core, wallet: &str) -> Vec<String> {
    use super::deal::Step;
    let ids: Vec<String> = lk(&core.inner.deals)
        .values()
        .filter(|d| !matches!(d.step, Step::Closed | Step::TakenOver))
        .map(|d| d.id.clone())
        .collect();
    ids.into_iter()
        .filter(|id| matches!(of_deal(core, id), Ok(Some(a)) if a.0 == wallet))
        .collect()
}

/// Решение 185 (Тигран, 05.11.2026, живой случай 20261105184401098DEFI kucoin → mexc): перегон застрявшей монеты
/// сделки биржа ↔ биржа (счёта DEX у сделки нет) на DEX шёл без кошелька — «Кошелёк DEX не выбран», сделка
/// останавливалась. Счёт для перегона на `to`: у сделки уже есть счёт — он; `to` не DEX — не нужен (None); `to` — DEX,
/// монета на бирже — кошелёк строки этой биржи в маппинге (решение 132: у каждой биржи свой кошелёк); строки нет —
/// Err (на DEX не перегонять). Принимает: ядро, сделку, площадку-цель. Ошибок сверх Err нет.
pub fn for_relay(core: &Core, d: &Deal, to: &VenueId) -> Result<Option<DexAccountId>, String> {
    if let Some(a) = &d.dex_account {
        return Ok(Some(a.clone()));
    }
    let kinds = rd(&core.inner.kinds);
    let kind = |v: &VenueId| kinds.iter().find(|(id, _)| id == v).map(|(_, k)| *k);
    if kind(to) != Some(VenueKind::Dex) {
        return Ok(None);
    }
    if kind(&d.at) != Some(VenueKind::Cex) {
        return Err(format!("перегон {} → {}: {NO_WALLET}", d.at.0, to.0));
    }
    match core.inner.deps.settings.accounts().row(&d.at.0) {
        Some(id) => Ok(Some(DexAccountId(id))),
        None => Err(format!("{}: {NO_WALLET}", d.at.0)),
    }
}
