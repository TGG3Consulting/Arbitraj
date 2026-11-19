//! Файл: core/gas_ticker.rs
//! Что делает: решение 416 (ревью шага 1, решение техлида) — тикер монеты газа сети из настройки площадки DEX
//! dex.gas_ticker (BNB у BSC, ETH у Arbitrum) вместо зашитого «BNB» (бывший core/convert.rs GAS_TICKER). Им ядро
//! узнаёт газ и чаевые в затратах (Cost.asset), читает баланс газа кошелька, подписывается на стакан газа бирж (цена
//! газа в USDT). dex.native_ticker (тикер нативной монеты на бирже, решения 62/270/276) — отдельная настройка, не
//! трогается. Решение 430 (шаг 2): газ — по сети: площадка DEX сети монеты (core/chain.rs dex_of) — её dex.gas_ticker;
//! бирже — стаканы газа всех сетей (all). Одна площадка DEX — её значение, как раньше. Не задана — газ-зависимые
//! функции не работают с точной причиной «Не задана настройка dex.gas_ticker …».
//! Кто вызывает: core/{convert_at,deal_gas,cycle_fact,precheck,buy_replan,native,balances_cache,sell,reconcile,subscribe,
//! gas_stats}.rs, report/snap_dex.rs, api/wallets_dex.rs.
//! От чего зависит: settings (store, read), core/chain.rs (площадка DEX сети), engine.rs.
//! Настройки: dex.gas_ticker (по площадке DEX).

use crate::core::types::{Asset, ChainId, VenueId};
use crate::settings::read::At;
use crate::settings::store::Store;

use super::engine::Core;

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

/// Что делает: тикер газа сети (канонический вид: None — сеть по умолчанию) — dex.gas_ticker площадки DEX этой сети.
/// Принимает: ядро, сеть. Возвращает: тикер. Ошибки: текст — площадки DEX сети нет (core/chain.rs dex_of), не задана.
pub fn of_chain(core: &Core, chain: Option<&ChainId>) -> Result<String, String> {
    let dex = super::chain::dex_of(
        &core.inner.deps.settings,
        &super::chain::dexes(core),
        chain,
    )?;
    at(&core.inner.deps.settings, &dex)
}

/// Что делает: тикер газа сети монеты (of_chain по Asset.chain). Принимает: ядро, монету. Ошибки: как у of_chain.
pub fn of_asset(core: &Core, a: &Asset) -> Result<String, String> {
    of_chain(core, a.chain.as_ref())
}

/// Что делает: тикеры газа всех площадок DEX, у которых dex.gas_ticker задана (без повторов, по порядку реестра) —
/// бирже нужны стаканы газа всех сетей. Принимает: ядро. Возвращает: список (пусто — ни у одной не задана).
/// Ошибок нет.
pub fn all(core: &Core) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for v in super::chain::dexes(core) {
        if let Ok(t) = at(&core.inner.deps.settings, &v)
            && !out.contains(&t)
        {
            out.push(t);
        }
    }
    out
}

/// Что делает: тикер газа сети площадок DEX, когда сеть неизвестна (решение 416): у всех площадок DEX один тикер — он.
/// Решение 430: ядро зовёт по сети (of_chain, of_asset, all); эта функция — для случая «сеть не известна».
/// Принимает: ядро. Возвращает: тикер. Ошибки: текст — площадок DEX нет, не задана, у площадок DEX разные тикеры газа.
pub fn of_core(core: &Core) -> Result<String, String> {
    let mut found: Option<String> = None;
    for v in super::chain::dexes(core) {
        let t = at(&core.inner.deps.settings, &v)?;
        match &found {
            Some(f) if *f != t => {
                return Err(format!(
                    "у площадок DEX разные тикеры газа ({f}, {t}) — газ без сети не выбрать"
                ));
            }
            _ => found = Some(t),
        }
    }
    found.ok_or_else(|| "площадок DEX нет — тикер газа (dex.gas_ticker) неизвестен".to_string())
}

/// Что делает: тикер газа для площадки (решение 416): площадка DEX — её dex.gas_ticker, иначе — of_core.
/// Принимает: ядро, площадку. Возвращает: тикер. Ошибки: как у at / of_core.
pub fn for_venue(core: &Core, v: &VenueId) -> Result<String, String> {
    if super::daily_venue::is_dex(core, &v.0) {
        at(&core.inner.deps.settings, v)
    } else {
        of_core(core)
    }
}

/// Что делает: тикер газа для площадки сделки (решение 430): площадка DEX — её dex.gas_ticker; биржа — газ сети
/// `chain` (сеть сделки; None — сеть по умолчанию). Принимает: ядро, площадку, сеть. Возвращает: тикер.
/// Ошибки: как у at / of_chain.
pub fn for_venue_in(core: &Core, v: &VenueId, chain: Option<&ChainId>) -> Result<String, String> {
    if super::daily_venue::is_dex(core, &v.0) {
        at(&core.inner.deps.settings, v)
    } else {
        of_chain(core, chain)
    }
}
