//! Файл: core/sell_turn_last.rs
//! Что делает: решение 415 (вынесено из sell_turn.rs — размер файла; ревью t200, С-2, С-3) — след прошлой продажи
//! ключа очереди продажи счёта (Last): конец, блок, пулы маршрута запроса и известен ли маршрут. Маршрут запроса —
//! фактический маршрут обмена, только если исполнял свой сборщик: у DEX при exec.via_kyber не «да» путь — Own
//! (venues/bsc_dex/trade_path.rs of), итог своего сборщика несёт исходный запрос (exec/result.rs); при exec.via_kyber
//! маршрут строит KyberSwap (kyberswap/trade_prep.rs), а итог несёт наш запрос (kyberswap/trade_fact.rs) — маршрут
//! «неизвестен» (route_known = false).
//! Проверка свежести котировки после следа (pools_fresh — до котировки, after — по котировке):
//! - биржа — стакан котировки новее конца прошлой продажи (Quote.at > Last.at);
//! - DEX, блок известен: (а) маршрут известен и все его пулы в памяти ядра уже с блоком ≥ её блока (адаптер шлёт пул
//!   ядру ПОСЛЕ записи в свой реестр — venues/bsc_dex/pools/emit_pool.rs); маршрут известен и котировка не через её
//!   пулы (PoolRef::site) — свежо; иначе (и всегда при неизвестном маршруте, С-3) — маршрут котировки по блоку ≥ её
//!   блока (DexRoute.block — наименьший блок пулов маршрута, quote/costs.rs);
//! - DEX, блок неизвестен (исход «неизвестен», С-2, или итог без блока): (а) по времени — пулы известного маршрута в
//!   памяти ядра получены позже её конца (Pool.received_at); иначе — маршрут котировки по блоку не раньше первого блока,
//!   пришедшего после её конца (наименьший Pool.block среди пулов площадки с received_at позже Last.at).
//!
//! Кто вызывает: core/sell_turn.rs (done), core/sell_turn_fresh.rs (quote_with).
//! От чего зависит: trade_ops.rs (вид площадки), ride_trail.rs (sites), market_state.rs (пулы ядра), engine.rs,
//! settings, core/types.
//! Настройки: читает exec.via_kyber [площадка DEX] (как адаптер: не задана — нет).

use chrono::{DateTime, Utc};

use crate::core::types::{DexRoute, PoolRef, Quote, TradeResult, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::engine::{Core, rd};

/// След прошлой исполненной продажи на ключе очереди.
#[derive(Debug, Clone, PartialEq)]
pub struct Last {
    /// Сделка, которая продала.
    pub deal: String,
    /// Конец продажи (ответ площадки) — у биржи котировка должна быть новее.
    pub at: DateTime<Utc>,
    /// DEX: блок транзакции продажи; None — неизвестен (у биржи — всегда None).
    pub block: Option<u64>,
    /// DEX: пулы маршрута продажи (как в запросе ордера); пусто — запрос без маршрута или биржа.
    pub pools: Vec<PoolRef>,
    /// DEX: маршрут запроса — фактический маршрут обмена (исполнял свой сборщик, С-3); false — неизвестен.
    pub known: bool,
}

/// Пулы маршрута (как в запросе). Ошибок нет.
fn pools_of(r: Option<&DexRoute>) -> Vec<PoolRef> {
    let mut pools: Vec<PoolRef> = r
        .iter()
        .flat_map(|x| x.legs.iter())
        .flat_map(|l| l.hops.iter().map(|h| h.pool.clone()))
        .collect();
    pools.dedup();
    pools
}

impl Last {
    /// Что делает: след по исполненной продаже. Принимает: итог ордера, известен ли маршрут (С-3). Возвращает: след.
    /// Ошибок нет.
    pub fn of(r: &TradeResult, known: bool) -> Self {
        Self {
            deal: r.request.deal_id.clone(),
            at: r.finished_at,
            block: r.chain.as_ref().and_then(|c| c.block),
            pools: pools_of(r.request.route.as_ref()),
            known,
        }
    }

    /// Что делает: след продажи с исходом «неизвестен» (С-2): конец — сейчас, блок неизвестен, пулы — из запроса.
    /// Принимает: ID сделки, маршрут запроса, известен ли маршрут. Возвращает: след. Ошибок нет.
    pub fn unknown(deal: &str, route: Option<&DexRoute>, known: bool) -> Self {
        Self {
            deal: deal.to_string(),
            at: Utc::now(),
            block: None,
            pools: pools_of(route),
            known,
        }
    }
}

/// Что делает: маршрут запроса продажи на площадке `v` — фактический (С-3): биржа — маршрута нет (true); DEX — только
/// если exec.via_kyber площадки не «да» (как читает адаптер: не задана — нет). Ошибок нет.
pub fn route_known(core: &Core, v: &VenueId) -> bool {
    if super::trade_ops::kind(core, v) != Ok(VenueKind::Dex) {
        return true;
    }
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    !core
        .inner
        .deps
        .settings
        .flag("exec.via_kyber", at)
        .unwrap_or(false)
}

/// Что делает: признак (а) — состояние пулов известного маршрута прошлой продажи в памяти ядра после неё (по блоку,
/// блок неизвестен — по времени получения; шапка). Снимается ДО котировки. Принимает: ядро, площадку, след.
/// Возвращает: да — после неё; нет — не доказано. Ошибок нет.
pub fn pools_fresh(core: &Core, v: &VenueId, last: &Last) -> bool {
    if !last.known || last.pools.is_empty() {
        return false;
    }
    let m = rd(&core.inner.market);
    last.pools.iter().all(|p| {
        m.pools
            .get(&(v.clone(), p.clone()))
            .is_some_and(|x| match last.block {
                Some(b) => x.block >= b,
                None => x.received_at > last.at,
            })
    })
}

/// Что делает: котировка `q` новее прошлой продажи (шапка). Принимает: ядро, площадку, след, признак (а), снятый до
/// котировки, котировку. Возвращает: да — свежая. Ошибок нет (площадки нет в реестре — не доказано).
pub fn after(core: &Core, v: &VenueId, last: &Last, early: bool, q: &Quote) -> bool {
    match super::trade_ops::kind(core, v) {
        Ok(VenueKind::Cex) => q.at > last.at,
        Ok(VenueKind::Dex) => {
            if early {
                return true;
            }
            let Some(r) = &q.route else {
                return false;
            };
            let Some(b) = last.block else {
                return first_after(core, v, last.at).is_some_and(|b0| r.block >= b0);
            };
            if last.known {
                let mine = super::ride_trail::sites(r);
                let theirs: Vec<PoolRef> = last.pools.iter().map(PoolRef::site).collect();
                if !theirs.is_empty() && !mine.iter().any(|p| theirs.contains(p)) {
                    return true;
                }
            }
            r.block >= b
        }
        Err(_) => false,
    }
}

/// Что делает: первый блок, пришедший после `t`: наименьший Pool.block среди пулов площадки в памяти ядра, полученных
/// позже `t`. Принимает: ядро, площадку, время. Возвращает: блок; None — ничего не пришло. Ошибок нет.
pub fn first_after(core: &Core, v: &VenueId, t: DateTime<Utc>) -> Option<u64> {
    let m = rd(&core.inner.market);
    m.pools
        .iter()
        .filter(|((pv, _), p)| pv == v && p.received_at > t)
        .map(|(_, p)| p.block)
        .min()
}
