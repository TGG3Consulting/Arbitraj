//! Файл: core/sell_turn_last.rs
//! Что делает: решение 415 (вынесено из sell_turn.rs — размер файла; ревью t200, С-2, С-3) — след прошлой продажи
//! ключа очереди продажи счёта (Last): конец, блок, пулы маршрута запроса и известен ли маршрут. Маршрут исполнения
//! может отличаться от запроса, когда у площадки включён внешний исполнитель шлюза (gateway/quote_stage.rs
//! final_external, правило решения 121 B5 — как core/profit.rs): тогда маршрут следа «неизвестен» (route_known = false).
//! Ревью 2 t200 (2а): след «исход неизвестен» ставится в момент итога ордера (done); если транзакция попадёт в блок
//! позже, следующая продажа может счесть котировку свежей — тогда от неё защищает только проверка цены площадкой перед
//! отправкой ордера (худшая цена запроса).
//! Проверка свежести котировки после следа (pools_fresh — до котировки, after — по котировке):
//! - биржа — стакан котировки новее конца прошлой продажи (Quote.at > Last.at);
//! - DEX, блок известен: (а) маршрут известен и все его пулы в памяти ядра (события пулов площадки) уже с блоком ≥ её
//!   блока; маршрут известен и котировка не через её пулы (PoolRef::site) — свежо; иначе (и всегда при неизвестном
//!   маршруте, С-3) — маршрут котировки по блоку ≥ её блока (DexRoute.block — по типу ядра «блок, по состоянию
//!   которого посчитан маршрут»). Ревью 2 (С-3б): у пула без событий блок в памяти не растёт, а номера обработанного
//!   площадкой блока (головы) ни в событиях шлюза (gateway/events.rs MarketEvent), ни в ядре нет — при неизвестном
//!   маршруте через тихий пул ожидание длится до feed.stale_after_ms (известный остаток, docs/TASKS.md);
//! - DEX, блок неизвестен (исход «неизвестен», С-2, или итог без блока): (а) по времени — пулы известного маршрута в
//!   памяти ядра получены позже её конца (Pool.received_at); иначе — маршрут котировки по блоку не раньше первого блока,
//!   пришедшего после её конца (наименьший Pool.block среди пулов площадки с received_at позже Last.at).
//!
//! Кто вызывает: core/sell_turn.rs (done), core/sell_turn_fresh.rs (quote_with).
//! От чего зависит: trade_ops.rs (вид площадки), ride_trail.rs (sites), market_state.rs (пулы ядра), engine.rs,
//! settings, core/types.
//! Настройки: своих нет; признак внешнего исполнителя — gateway/quote_stage.rs final_external.

use chrono::{DateTime, Utc};

use crate::core::types::{DexRoute, PoolRef, Quote, TradeResult, VenueId};
use crate::gateway::contract::VenueKind;

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
/// если у площадки нет внешнего исполнителя шлюза (gateway/quote_stage.rs final_external). Ошибок нет.
pub fn route_known(core: &Core, v: &VenueId) -> bool {
    super::trade_ops::kind(core, v) != Ok(VenueKind::Dex)
        || !crate::gateway::quote_stage::final_external(&core.inner.deps.settings, v)
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
