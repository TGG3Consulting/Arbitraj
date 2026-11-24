//! Единицы конкретных торговых площадок и конкретной сети. Данные контракта
//! не определяют единицу биржевого ордера или депозита. Подтверждение площадки
//! используется только для операции, для которой сверены исполнение и перевод.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use tokio::sync::mpsc;

use super::engine::{Core, lk, rd};
use super::settlement_history::{self as history, Basis, Modes, Observation};
use super::types::{Asset, Direction, VenueId};
pub use super::unit_ratio::UnitRatio;
#[cfg(test)]
#[path = "settlement_legacy_replay.rs"]
mod legacy_replay;
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

type Sources = HashMap<VenueId, (Arc<dyn crate::gateway::contract::Venue>, Asset, String)>;
type Declarations = HashMap<(VenueId, bool), Result<Option<Decimal>, String>>;

#[derive(Clone)]
struct Snapshot {
    height: u64,
    decimals: u8,
    multiplier: Option<Decimal>,
    modes: Arc<Modes>,
    legacy: Arc<history::Legacy>,
    declared: Arc<Declarations>,
    sources: Arc<Sources>,
    revision: usize,
    expires: Option<Instant>,
    pending_evidence: bool,
}

#[derive(Default)]
struct Record {
    snapshot: Option<Snapshot>,
    observations: Vec<Observation>,
    receipts: Arc<history::Receipts>,
    scheduled: bool,
    retry: Option<Instant>,
}

#[derive(Default)]
pub struct Book {
    rows: Mutex<HashMap<Asset, Record>>,
    queue: OnceLock<mpsc::UnboundedSender<Asset>>,
}

pub(super) fn observe(core: &Core, deal: &super::deal::Deal) {
    if deal.transfers.len() < 2 {
        return;
    }
    let dir = &deal.direction;
    let kinds = rd(&core.inner.kinds);
    let Some((dex, _)) = kinds
        .iter()
        .find(|(v, k)| *k == VenueKind::Dex && (v == &dir.buy_on || v == &dir.sell_on))
    else {
        return;
    };
    let Some(fact) = history::observed(deal, dex) else {
        return;
    };
    drop(kinds);
    let mut rows = lk(&core.inner.settlement.rows);
    let r = rows.entry(dir.asset.clone()).or_default();
    if !r.observations.contains(&fact) {
        r.observations.push(fact);
        r.retry = None;
    }
}

pub(super) fn start(core: &Core) {
    for d in lk(&core.inner.deals).values() {
        observe(core, d);
    }
    let (tx, mut rx) = mpsc::unbounded_channel();
    if core.inner.settlement.queue.set(tx).is_err() {
        return;
    }
    let weak = Arc::downgrade(&core.inner);
    tokio::spawn(async move {
        while let Some(asset) = rx.recv().await {
            let Some(inner) = weak.upgrade() else { break };
            let core = Core { inner };
            let dex = super::chain::dex_of_asset(&core, &asset);
            let result = match &dex {
                Ok(v) => refresh(&core, &asset, v).await,
                Err(e) => Err(e.clone()),
            };
            let mut rows = lk(&core.inner.settlement.rows);
            let row = rows.entry(asset.clone()).or_default();
            row.scheduled = false;
            if result.is_err() {
                row.retry = dex.ok().and_then(|v| refresh_after(&core, &v, false));
            }
        }
    });
}

fn refresh_after(core: &Core, dex: &VenueId, ordinary: bool) -> Option<Instant> {
    let settings = &core.inner.deps.settings;
    if ordinary {
        return settings
            .integer("tax.max_age_s", At::default())
            .ok()
            .and_then(|sec| Instant::now().checked_add(Duration::from_secs(sec)));
    }
    let at = At {
        venue: Some(&dex.0),
        coin: None,
    };
    settings
        .integer("feed.stale_after_ms", at)
        .ok()
        .and_then(|ms| Instant::now().checked_add(Duration::from_millis(ms)))
}

fn leg(
    snapshot: &Snapshot,
    v: &VenueId,
    kind: VenueKind,
    withdrawal: bool,
) -> Result<Decimal, String> {
    let m = snapshot.multiplier.unwrap_or(Decimal::ONE);
    if kind == VenueKind::Dex {
        return Ok(Decimal::ONE);
    }
    let key = (v.clone(), withdrawal);
    let fresh;
    let declared = if let Some((venue, asset, network)) = snapshot.sources.get(v) {
        fresh = venue.market_units_per_raw(asset, network, withdrawal)?;
        &fresh
    } else {
        snapshot
            .declared
            .get(&key)
            .ok_or_else(|| {
                format!(
                    "{}: нет подтверждённой строки сети для проверки единиц",
                    v.0
                )
            })?
            .as_ref()
            .map_err(Clone::clone)?
    };
    if let Some(unit) = declared {
        if *unit <= Decimal::ZERO {
            return Err(format!("{}: недопустимая единица площадки", v.0));
        }
        if m == Decimal::ONE {
            return Ok(*unit);
        }
        let observed = match snapshot.modes.get(&key) {
            Some(Basis::Raw) => Some(Decimal::ONE),
            Some(Basis::Display) => Some(m),
            _ => None,
        };
        if observed == Some(*unit) {
            return Ok(*unit);
        }
        return Err(format!(
            "{}: сочетание биржевого лота {unit} и множителя контракта {m} требует сверки перевода",
            v.0
        ));
    }
    if m == Decimal::ONE {
        return Ok(Decimal::ONE);
    }
    match snapshot.modes.get(&(v.clone(), withdrawal)) {
        Some(Basis::Display) => Ok(m),
        Some(Basis::Raw) => Ok(Decimal::ONE),
        _ => Err(format!(
            "{}: единица спота и {} по этому контракту не подтверждена для коэффициента {m}",
            v.0,
            if withdrawal {
                "вывода"
            } else {
                "зачисления"
            }
        )),
    }
}

fn route(core: &Core, d: &Direction, s: &Snapshot) -> Result<UnitRatio, String> {
    let a = leg(s, &d.buy_on, super::trade_ops::kind(core, &d.buy_on)?, true)?;
    let b = leg(
        s,
        &d.sell_on,
        super::trade_ops::kind(core, &d.sell_on)?,
        false,
    )?;
    if a == Decimal::ONE && b == Decimal::ONE {
        return Ok(UnitRatio::IDENTITY);
    }
    Ok(UnitRatio {
        buy_per_raw: a,
        sell_per_raw: b,
        raw_decimals: Some(s.decimals),
    })
}

/// В расчёте только память. Для ещё неизвестного контракта запускается чтение независимо
/// от знака первого спреда; подтверждение при входе обязательно до выставления ордера.
pub fn ratio(core: &Core, d: &Direction) -> Result<UnitRatio, String> {
    let mut rows = lk(&core.inner.settlement.rows);
    let r = rows.entry(d.asset.clone()).or_default();
    let now = Instant::now();
    let due = r
        .snapshot
        .as_ref()
        .is_none_or(|s| s.expires.is_some_and(|t| now >= t) || s.revision != r.observations.len());
    if due
        && !r.scheduled
        && r.retry.is_none_or(|t| now >= t)
        && let Some(q) = core.inner.settlement.queue.get()
    {
        r.scheduled = q.send(d.asset.clone()).is_ok();
    }
    let snap = r.snapshot.clone();
    drop(rows);
    snap.map_or(Ok(UnitRatio::IDENTITY), |s| route(core, d, &s))
}

async fn declarations(core: &Core, asset: &Asset) -> (Declarations, Sources) {
    let places: Vec<_> = {
        let coins = rd(&core.inner.coins);
        super::ticker_index::find(core, &coins, asset)
            .into_iter()
            .flat_map(|c| c.presence.iter())
            .filter_map(|p| {
                p.network
                    .as_ref()
                    .map(|network| (p.venue.clone(), p.ticker.clone(), network.clone()))
            })
            .collect()
    };
    let mut declared = Declarations::new();
    let mut sources = Sources::new();
    for (id, ticker, network) in places {
        let named = Asset {
            ticker,
            ..asset.clone()
        };
        let venue = core.inner.deps.gw.get(&id).await;
        for withdrawal in [true, false] {
            let value = match &venue {
                Ok(v) => v.market_units_per_raw(&named, &network, withdrawal),
                Err(e) => Err(e.message.clone()),
            };
            declared.insert((id.clone(), withdrawal), value);
        }
        if let Ok(venue) = venue {
            sources.insert(id, (venue, named, network));
        }
    }
    (declared, sources)
}

async fn refresh(core: &Core, asset: &Asset, dex: &VenueId) -> Result<Snapshot, String> {
    let token = asset.contract.ok_or("нет контракта для проверки единиц")?;
    let venue = core.inner.deps.gw.get(dex).await.map_err(|e| e.message)?;
    let (height, decimals, multiplier) = venue
        .settlement_profile(token)
        .await
        .map_err(|e| e.message)?;
    let (previous, revision, pending, receipts) = {
        let rows = lk(&core.inner.settlement.rows);
        let row = rows.get(asset);
        let prev = row.and_then(|r| r.snapshot.clone());
        let rev = row.map_or(0, |r| r.observations.len());
        let need = multiplier.is_some_and(|m| m != Decimal::ONE)
            && prev.as_ref().is_none_or(|p| {
                p.multiplier != multiplier
                    || p.revision != rev
                    || (p.pending_evidence && p.expires.is_none_or(|t| t <= Instant::now()))
            });
        (
            prev,
            rev,
            need.then(|| row.map(|r| r.observations.clone()).unwrap_or_default()),
            row.map(|r| r.receipts.clone()).unwrap_or_default(),
        )
    };
    let (modes, legacy, pending_evidence) = match pending {
        Some(obs) => {
            let (m, legacy, p) = history::verified(
                core,
                asset,
                multiplier.unwrap_or(Decimal::ONE),
                &obs,
                &receipts,
            )
            .await;
            (Arc::new(m), Arc::new(legacy), p)
        }
        None => previous
            .as_ref()
            .map(|p| (p.modes.clone(), p.legacy.clone(), p.pending_evidence))
            .unwrap_or_default(),
    };
    let (declared, sources) = declarations(core, asset).await;
    let declared = Arc::new(declared);
    let changed = previous.as_ref().is_none_or(|p| {
        p.multiplier != multiplier
            || *p.modes != *modes
            || *p.legacy != *legacy
            || *p.declared != *declared
    });
    let snapshot = Snapshot {
        height,
        decimals,
        multiplier,
        modes,
        legacy,
        declared: declared.clone(),
        sources: Arc::new(sources),
        revision,
        pending_evidence,
        expires: refresh_after(core, dex, multiplier.is_none()),
    };
    {
        let mut rows = lk(&core.inner.settlement.rows);
        let r = rows.entry(asset.clone()).or_default();
        if let Some(newer) = &r.snapshot
            && (newer.height > height || (newer.height == height && newer.revision > revision))
        {
            return Ok(newer.clone());
        }
        r.snapshot = Some(snapshot.clone());
        r.retry = None;
    }
    if changed
        && (previous.is_some()
            || multiplier.is_some_and(|m| m != Decimal::ONE)
            || declared.values().any(|v| matches!(v, Ok(Some(_)))))
    {
        let venues: HashSet<_> = rd(&core.inner.kinds)
            .iter()
            .map(|(v, _)| v.clone())
            .collect();
        super::tick_sched::schedule_set(core, asset.clone(), venues);
    }
    Ok(snapshot)
}

pub(super) fn confirmed_cached(core: &Core, d: &Direction) -> Result<UnitRatio, String> {
    if super::native::is_native(core, &d.asset)
        || (super::trade_ops::kind(core, &d.buy_on)? == VenueKind::Dex
            && super::trade_ops::kind(core, &d.sell_on)? == VenueKind::Dex)
    {
        return Ok(UnitRatio::IDENTITY);
    }
    let now = Instant::now();
    let snap = {
        let rows = lk(&core.inner.settlement.rows);
        let record = rows
            .get(&d.asset)
            .ok_or("нет подтверждённого снимка единиц")?;
        record
            .snapshot
            .as_ref()
            .filter(|s| {
                s.expires.is_none_or(|t| t > now) && s.revision == record.observations.len()
            })
            .cloned()
            .ok_or("ожидается свежий снимок единиц")?
    };
    route(core, d, &snap)
}

/// Подтверждения берутся из опубликованного снимка. Здесь нет постановки задания или сетевого вызова.
pub(super) fn interpreted_legacy(
    core: &Core,
    seen: &super::withdraw_seen::Seen,
    to: Option<&VenueId>,
) -> history::Interpretation {
    let rows = lk(&core.inner.settlement.rows);
    rows.get(&seen.asset)
        .and_then(|row| {
            row.snapshot
                .as_ref()
                .filter(|s| s.revision == row.observations.len())
        })
        .map_or(history::Interpretation::Unknown, |s| {
            s.legacy.interpreted(seen, to)
        })
}

pub(super) async fn confirmed(core: &Core, d: &Direction) -> Result<UnitRatio, String> {
    if super::native::is_native(core, &d.asset) {
        return Ok(UnitRatio::IDENTITY);
    }
    let a = super::trade_ops::kind(core, &d.buy_on)?;
    let b = super::trade_ops::kind(core, &d.sell_on)?;
    let dex = match (a, b) {
        (VenueKind::Dex, VenueKind::Dex) => return Ok(UnitRatio::IDENTITY),
        (VenueKind::Dex, _) => d.buy_on.clone(),
        (_, VenueKind::Dex) => d.sell_on.clone(),
        _ => super::chain::dex_of_asset(core, &d.asset)?,
    };
    let snap = refresh(core, &d.asset, &dex).await?;
    route(core, d, &snap)
}
