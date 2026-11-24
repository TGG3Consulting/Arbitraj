//! Наблюдения в раздельных единицах привязаны к площадке получения. Факт 1:1
//! из прежнего учёта исключается только при совпадении всех сохранённых величин.

use super::Seen;
use crate::core::{
    engine::Core,
    persist,
    types::{Asset, VenueId},
};
use crate::storage::journal::Level;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

const KIND: &str = "withdraw_route_fact";
type Origin = (Asset, VenueId);
type Route = (Asset, VenueId, VenueId);

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::core) struct Receipt {
    pub tx: String,
    pub raw: Decimal,
    pub requested: Decimal,
    /// Есть у пересчёта по подтверждённой единице конкретного направления.
    /// None — суммы взяты непосредственно из пары событий одной квитанции.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub venue_units: Option<crate::core::settlement_unit::UnitRatio>,
}

#[cfg(test)]
#[path = "withdraw_units_tests.rs"]
mod tests;

impl Receipt {
    fn matches(&self, old: &Seen, converted: &Seen) -> bool {
        old.asset == converted.asset
            && old.from == converted.from
            && old.fee == converted.fee
            && (old.sent == self.requested || old.sent == converted.sent)
            && old.credited == self.raw
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Row {
    to: VenueId,
    fact: Seen,
    receipt: Option<Receipt>,
    // Даже если позже поступила монета с настоящим налогом, старый доказанный
    // дубликат в withdraw_fact нельзя выдавать другому получателю как недостачу.
    replaced: Option<(Seen, String)>,
}

#[derive(Default)]
struct State {
    legacy: HashMap<Origin, Seen>,
    routes: HashMap<Route, Seen>,
    replaced: HashMap<Origin, (Seen, String)>,
}

impl State {
    fn lookup(&self, asset: &Asset, from: &VenueId, to: Option<&VenueId>) -> Option<Seen> {
        self.source(asset, from, to).map(|(s, _)| s)
    }

    fn source(&self, asset: &Asset, from: &VenueId, to: Option<&VenueId>) -> Option<(Seen, bool)> {
        if let Some(to) = to
            && !self.routes.is_empty()
            && let Some(s) = self.routes.get(&(asset.clone(), from.clone(), to.clone()))
        {
            return Some((s.clone(), false));
        }
        let key = (asset.clone(), from.clone());
        let fact = self.legacy.get(&key)?;
        self.replaced
            .get(&key)
            .is_none_or(|(s, _)| s != fact)
            .then(|| (fact.clone(), true))
    }
    fn put(&mut self, row: &Row) {
        let s = &row.fact;
        self.routes
            .insert((s.asset.clone(), s.from.clone(), row.to.clone()), s.clone());
        if let Some(replaced) = &row.replaced {
            self.replaced
                .insert((s.asset.clone(), s.from.clone()), replaced.clone());
        }
    }
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

pub(super) async fn load(core: &Core) -> bool {
    match persist::load::<Row>(&core.inner.deps.db, KIND).await {
        Ok(rows) => {
            let mut state = STATE.lock().unwrap_or_else(|p| p.into_inner());
            for row in rows {
                state.put(&row);
            }
            true
        }
        Err(e) => {
            core.inner
                .deps
                .log(
                    Level::Error,
                    "core.withdraw_seen",
                    None,
                    format!("Не удалось загрузить единицы фактов по маршрутам: {e}"),
                    None,
                )
                .await;
            false
        }
    }
}

pub(super) fn put_legacy(fact: Seen) {
    let key = (fact.asset.clone(), fact.from.clone());
    STATE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .legacy
        .insert(key, fact);
}

pub(super) fn lookup(asset: &Asset, from: &VenueId, to: Option<&VenueId>) -> Option<Seen> {
    STATE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .lookup(asset, from, to)
}

pub(in crate::core) fn get_for(
    core: &Core,
    asset: &Asset,
    from: &VenueId,
    to: Option<&VenueId>,
) -> Option<Seen> {
    use crate::core::settlement_history::Interpretation;
    let (fact, legacy) = STATE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .source(asset, from, to)?;
    if legacy && fact.fee.is_some_and(|f| fact.sent - f > fact.credited) {
        match crate::core::settlement_unit::interpreted_legacy(core, &fact, to) {
            Interpretation::ThisRoute(converted) => return Some(converted),
            Interpretation::OtherRoute => return None,
            Interpretation::Unknown => {}
        }
    }
    Some(fact)
}

pub(in crate::core) async fn record_for(
    core: &Core,
    to: &VenueId,
    fact: Seen,
    receipt: Option<Receipt>,
) {
    let origin = (fact.asset.clone(), fact.from.clone());
    let key = (fact.asset.clone(), fact.from.clone(), to.clone());
    let scoped = {
        let mut state = STATE.lock().unwrap_or_else(|p| p.into_inner());
        if receipt.is_some() || state.routes.contains_key(&key) {
            let replaced = receipt
                .as_ref()
                .and_then(|r| {
                    let old = state.legacy.get(&origin)?;
                    r.matches(old, &fact).then(|| (old.clone(), r.tx.clone()))
                })
                .or_else(|| state.replaced.get(&origin).cloned());
            let row = Row {
                to: to.clone(),
                fact: fact.clone(),
                receipt,
                replaced,
            };
            state.put(&row);
            Some(row)
        } else {
            None
        }
    };
    let Some(row) = scoped else {
        super::record(core, fact).await;
        return;
    };
    let rid = format!("{}:{}", super::id(&fact.asset, &fact.from), to.0);
    if let Err(e) = persist::save(&core.inner.deps.db, KIND, &rid, &row).await {
        core.inner
            .deps
            .log(
                Level::Error,
                "core.withdraw_seen",
                None,
                format!("Факт перевода {rid} с указанием единиц не сохранён: {e}"),
                None,
            )
            .await;
    }
}
