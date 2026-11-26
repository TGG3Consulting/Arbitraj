//! Файл: storage/cex_funds_watch.rs.
//! Назначение: выбирает обычный путь без SQL либо защищённый путь по состоянию конкретной площадки.
//! Потребители: денежный вход и писатель учёта брони. Состояние восстанавливается до запуска исполнителей.
//! Зависимости: SQLite при загрузке; в обычном пути только локальный счётчик до выхода из адаптера.
//! Настройки: отсутствуют.
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, RwLock},
};
type Id = [u8; 32];
#[derive(Default)]
struct State {
    known: HashSet<Id>,
    mandates: HashSet<Id>,
    active: HashSet<Id>,
    legacy: HashMap<String, Arc<Legacy>>,
    guarded: usize,
    openings: HashMap<String, Opening>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Opening {
    pub owner: String,
    pub currency: String,
    pub cap: rust_decimal::Decimal,
}
#[derive(Clone, Default)]
pub struct Watch(Arc<RwLock<HashMap<String, Arc<Mutex<State>>>>>);
#[path = "cex_funds_legacy.rs"]
mod legacy;
pub use legacy::{Legacy, Prior};
pub struct Lease {
    state: Arc<Mutex<State>>,
    pub guarded: bool,
    pub known: bool,
    pub legacy: Option<Arc<Legacy>>,
    pub prior: Vec<Prior>,
    key: String,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if self.guarded {
            s.guarded -= 1
        } else if let Some(record) = s.legacy.get(&self.key) {
            let remove = !record.prepared();
            if remove {
                s.legacy.remove(&self.key);
            }
        }
    }
}
impl Watch {
    /// Открытие настоящей основной БД ждёт загрузки. Повреждённый номер валюты не превращается в пустой кэш.
    pub fn restore(c: &Connection) -> rusqlite::Result<Self> {
        let out = Self::default();
        let mut q=c.prepare("SELECT json_extract(f.bucket,'$[0]'),f.action,f.debit,s.action FROM cex_funds f LEFT JOIN cex_funds_seen s ON s.action=f.action")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let venue: String = r.get(0)?;
            let action: String = r.get(1)?;
            let debit: Option<String> = r.get(2)?;
            let active = match debit {
                None => true,
                Some(s) => !s
                    .parse::<rust_decimal::Decimal>()
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?
                    .is_zero(),
            };
            let observed = r.get::<_, Option<String>>(3)?.is_some();
            out.changed(&venue, &action, Some(active && !observed));
        }
        let mut q=c.prepare("SELECT json_extract(d.body,'$.direction.buy_on'),json_extract(j.value,'$.action') FROM core_state d INDEXED BY cash_legacy_orders JOIN json_each(d.body,'$.plan.cash_legacy_ioc') j WHERE d.kind='deal' AND json_valid(d.body) AND json_extract(d.body,'$.plan.cash_legacy_ioc') IS NOT NULL")?;
        let mut archived = q.query([])?;
        while let Some(r) = archived.next()? {
            let venue: String = r.get(0)?;
            let action: String = r.get(1)?;
            let state = out.venue(&venue);
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .known
                .insert(Sha256::digest(action).into());
        }
        let mut q=c.prepare(&format!("SELECT id,json_extract(body,'$.plan.settlement_cycle.market.cex'),json_extract(body,'$.step') FROM core_state INDEXED BY cex_funds_held_deals WHERE {} AND json_valid(body)",super::cex_funds::HOLD_FILTER))?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            if let (Some(venue), Some(step)) = (
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ) {
                let id: String = r.get(0)?;
                out.mandate(
                    &venue,
                    &id,
                    !matches!(step.as_str(), "Closed" | "Halted" | "TakenOver"),
                );
            }
        }
        Ok(out)
    }
    /// Снимок поступает в то же штатное сохранение сделки, что и итог обычной покупки.
    pub(crate) fn finished_attempts(&self, venue: &str, deal: &str) -> Vec<Prior> {
        let state = self.venue(venue);
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        s.legacy
            .iter()
            .filter_map(|(k, r)| r.snapshot(k, false))
            .filter(|r| r.request.deal_id == deal)
            .collect()
    }
    pub(crate) fn archived(&self, venue: &str, keys: &[String]) {
        let state = self.venue(venue);
        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
        for k in keys {
            s.known.insert(Sha256::digest(k.as_bytes()).into());
            s.legacy.remove(k);
        }
    }
    pub(crate) fn opening(&self, venue: &str, deal: &str, entry: Option<Opening>) {
        let state = self.venue(venue);
        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entry {
            s.openings.insert(deal.to_string(), entry);
        } else {
            s.openings.remove(deal);
        }
    }
    pub(crate) fn saved(&self, venue: &str, deal: &str) -> Option<Opening> {
        self.venue(venue)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .openings
            .get(deal)
            .cloned()
    }
    pub(crate) fn mandate(&self, venue: &str, id: &str, open: bool) {
        let v = self.venue(venue);
        let mut s = v.lock().unwrap_or_else(|p| p.into_inner());
        let key: Id = Sha256::digest(id.as_bytes()).into();
        if open {
            s.mandates.insert(key);
        } else {
            s.mandates.remove(&key);
        }
    }
    pub fn unresolved(&self, venue: &str) -> bool {
        self.0
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(venue)
            .is_some_and(|s| {
                !s.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .active
                    .is_empty()
            })
    }
    fn venue(&self, name: &str) -> Arc<Mutex<State>> {
        if let Some(v) = self.0.read().unwrap_or_else(|p| p.into_inner()).get(name) {
            return v.clone();
        }
        self.0
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .entry(name.into())
            .or_default()
            .clone()
    }
    pub(super) fn changed(&self, venue: &str, action: &str, active: Option<bool>) {
        let id = Sha256::digest(action.as_bytes()).into();
        let v = self.venue(venue);
        let mut s = v.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(active) = active {
            s.known.insert(id);
            if active {
                s.active.insert(id);
            } else {
                s.active.remove(&id);
            }
        } else {
            s.known.remove(&id);
            s.active.remove(&id);
        }
    }
    /// Предыдущая обычная отправка уже могла уйти без сохраняемого ID. Начать смешанную одновременно нельзя.
    /// В установившемся режиме параллельные защищённые операции сериализуются только на проверку своего bucket в БД.
    pub fn enter(
        &self,
        venue: &str,
        action: &str,
        required: bool,
        request: &crate::core::types::TradeRequest,
        signer: &str,
    ) -> Result<Lease, String> {
        let state = self.venue(venue);
        let id: Id = Sha256::digest(action.as_bytes()).into();
        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
        let known = s.known.contains(&id) || s.legacy.get(action).is_some_and(|r| r.prepared());
        let guarded = required
            || known
            || !s.mandates.is_empty()
            || !s.active.is_empty()
            || s.guarded != 0
            || s.legacy.values().any(|r| r.transitioned());
        let (legacy, prior) = if guarded {
            s.guarded += 1;
            (
                None,
                s.legacy
                    .iter()
                    .filter_map(|(k, r)| r.snapshot(k, true))
                    .collect(),
            )
        } else {
            if s.legacy.contains_key(action) {
                return Err("Обычный ордер с тем же действием ещё выполняется".into());
            }
            let rec = Arc::new(Legacy::new(request.clone(), signer.to_string()));
            s.legacy.insert(action.to_string(), rec.clone());
            (Some(rec), vec![])
        };
        drop(s);
        Ok(Lease {
            state,
            guarded,
            known,
            legacy,
            prior,
            key: action.to_string(),
        })
    }
}
