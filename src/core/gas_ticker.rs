//! Файл: core/gas_ticker.rs
//! Что делает: решение 416 (ревью шага 1, решение техлида) — тикер монеты газа сети из настройки площадки DEX
//! dex.gas_ticker (BNB у BSC, ETH у Arbitrum) вместо зашитого «BNB» (бывший core/convert.rs GAS_TICKER). Им ядро
//! узнаёт газ и чаевые в затратах (Cost.asset), читает баланс газа кошелька, подписывается на стакан газа бирж (цена
//! газа в USDT). dex.native_ticker (тикер нативной монеты на бирже, решения 62/270/276) — отдельная настройка, не
//! трогается. Пока площадка DEX одна — её значение; площадки DEX в разных сетях — тикер по площадке (at) или ошибка «у
//! площадок DEX разные тикеры газа» там, где площадка неизвестна (шаг 2: по сети монеты/сделки). Не задана — газ-зависимые
//! функции не работают с точной причиной «Не задана настройка dex.gas_ticker …».
//! Кто вызывает: core/{convert_at,deal_gas,cycle_fact,precheck,buy_replan,native,balances_cache,sell,reconcile,subscribe,
//! gas_stats}.rs, report/snap_dex.rs, api/wallets_dex.rs.
//! От чего зависит: settings (store, read), engine.rs (виды площадок).
//! Настройки: dex.gas_ticker (по площадке DEX).

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::engine::{Core, rd};

/// Что делает: тикер газа площадки DEX. Принимает: хранилище, площадку DEX. Возвращает: тикер.
/// Ошибки: текст — не задана или пуста.
pub fn at(s: &Store, v: &VenueId) -> Result<String, String> {
    let a = At {
        venue: Some(&v.0),
        coin: None,
    };
    let t = s.text("dex.gas_ticker", a).map_err(|e| e.message)?;
    if t.trim().is_empty() {
        return Err(format!(
            "{}: dex.gas_ticker пуста — задайте тикер газа сети",
            v.0
        ));
    }
    Ok(t)
}

/// Что делает: тикер газа сети площадок DEX ядра (виды реестра). Принимает: ядро. Возвращает: тикер.
/// Ошибки: текст — площадок DEX нет, не задана, у площадок DEX разные тикеры газа.
pub fn of_core(core: &Core) -> Result<String, String> {
    let dex: Vec<VenueId> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
        .map(|(v, _)| v.clone())
        .collect();
    let mut found: Option<String> = None;
    for v in &dex {
        let t = at(&core.inner.deps.settings, v)?;
        match &found {
            Some(f) if *f != t => {
                return Err(format!(
                    "у площадок DEX разные тикеры газа ({f}, {t}) — газ без площадки не выбрать (шаг 2)"
                ));
            }
            _ => found = Some(t),
        }
    }
    found.ok_or_else(|| "площадок DEX нет — тикер газа (dex.gas_ticker) неизвестен".to_string())
}

/// Что делает: тикер газа для площадки сделки: площадка DEX — её dex.gas_ticker, иначе — сети площадок DEX (of_core).
/// Принимает: ядро, площадку. Возвращает: тикер. Ошибки: как у at / of_core.
pub fn for_venue(core: &Core, v: &VenueId) -> Result<String, String> {
    let dex = rd(&core.inner.kinds)
        .iter()
        .any(|(x, k)| x == v && *k == VenueKind::Dex);
    if dex {
        at(&core.inner.deps.settings, v)
    } else {
        of_core(core)
    }
}
