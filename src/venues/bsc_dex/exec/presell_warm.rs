//! Только выбранная предпродажа держит интерес к паре кошелёк/токен. При
//! отмене или выходе из предпроверки последний владелец отменяет фоновое чтение.
mod worker;
use super::{presell_layout as layout, presell_slot::Slot};
use alloy_primitives::{Address, B256, U256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, OnceCell};
pub(in crate::venues::bsc_dex) use worker::candidate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    job: u64,
    generation: u64,
    number: u64,
    pub(super) hash: B256,
}
type Funds = Result<(layout::Identity, Slot, U256), String>;
type JobKey = (Address, Address, u64);
type Binding = OnceCell<Result<Option<layout::Identity>, String>>;
struct Record {
    stamp: Stamp,
    funds: Funds,
    retry_used: bool,
}
struct Job {
    id: u64,
    interests: usize,
    record: Option<Record>,
    wake: Arc<Notify>,
    task: Option<tokio::task::AbortHandle>,
    last_error: Option<String>,
}
#[derive(Default)]
struct State {
    starts: u64,
    joins: u64,
    hits: u64,
    misses: u64,
    fallback_short: u64,
    recovered: u64,
    next: u64,
    jobs: HashMap<JobKey, Job>,
    recent: VecDeque<(JobKey, Record)>,
    deployments: HashMap<Address, ((u64, B256), Arc<Binding>)>,
}
/// Ограничиваем лишь старые записи без активной предпроверки. Все живые
/// интересы обслуживаются штатной очередью узла с её долей фона.
const IDLE_PROOFS: usize = 32;
#[derive(Default)]
pub(in crate::venues::bsc_dex) struct Cache {
    state: Mutex<State>,
}
enum Admission {
    Existing(u64),
    New(u64, Arc<Notify>),
}
impl State {
    fn admit(&mut self, key: JobKey) -> Admission {
        if let Some(j) = self.jobs.get_mut(&key) {
            self.joins = self.joins.saturating_add(1);
            j.interests += 1;
            return Admission::Existing(j.id);
        }
        self.starts = self.starts.saturating_add(1);
        self.next = self.next.wrapping_add(1);
        let wake = Arc::new(Notify::new());
        let record = self
            .recent
            .iter()
            .position(|(k, _)| *k == key)
            .and_then(|i| self.recent.remove(i))
            .map(|(_, mut r)| {
                r.stamp.job = self.next;
                r
            });
        self.jobs.insert(
            key,
            Job {
                id: self.next,
                interests: 1,
                record,
                wake: wake.clone(),
                task: None,
                last_error: None,
            },
        );
        Admission::New(self.next, wake)
    }
    fn release(&mut self, key: JobKey, id: u64) {
        if let Some(j) = self.jobs.get_mut(&key)
            && j.id == id
        {
            j.interests -= 1;
            if j.interests == 0 {
                if let Some(job) = self.jobs.remove(&key) {
                    if let Some(task) = job.task {
                        task.abort();
                    }
                    if let Some(r) = job.record {
                        self.recent.push_back((key, r));
                        if self.recent.len() > IDLE_PROOFS {
                            self.recent.pop_front();
                        }
                    }
                }
                if !self.jobs.keys().any(|(_, token, _)| *token == key.1) {
                    self.deployments.remove(&key.1);
                }
            }
        }
    }
}
impl Cache {
    pub(super) fn read(
        &self,
        wallet: Address,
        t: Address,
        n: u64,
        generation: u64,
    ) -> Option<Result<(Stamp, Slot, U256), String>> {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let key = (wallet, t, generation);
        let job = s.jobs.get(&key);
        let record = job
            .and_then(|j| j.record.as_ref())
            .or_else(|| s.recent.iter().find(|(k, _)| *k == key).map(|(_, r)| r));
        let result = match record {
            Some(r) if r.stamp.number == n => {
                Some(r.funds.clone().map(|(_, slot, b)| (r.stamp, slot, b)))
            }
            Some(_) => Some(Err(job
                .and_then(|j| j.last_error.clone())
                .unwrap_or_else(|| "фоновая проверка относится к другому блоку".into()))),
            None if layout::seed(t) => Some(Err(job
                .and_then(|j| j.last_error.clone())
                .unwrap_or_else(|| "фоновая проверка баланса ещё не готова".into()))),
            None => None,
        };
        match &result {
            Some(Ok(_)) => s.hits = s.hits.saturating_add(1),
            Some(Err(_)) => s.misses = s.misses.saturating_add(1),
            None => {}
        }
        result
    }
    pub(super) fn funding_short(&self, recovered: bool) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if recovered {
            s.recovered = s.recovered.saturating_add(1)
        } else {
            s.fallback_short = s.fallback_short.saturating_add(1)
        }
    }
    pub(super) fn totals(&self) -> String {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        format!(
            "предпроверки из кэша {}, без готовой записи {}, фоновых задач {}, объединено {}, нехваток источников {}, возвращено полных проверок {}",
            s.hits, s.misses, s.starts, s.joins, s.fallback_short, s.recovered
        )
    }
    pub(super) fn reject(&self, wallet: Address, t: Address, stamp: Stamp, why: &str) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(j) = s.jobs.get_mut(&(wallet, t, stamp.generation))
            && let Some(r) = &mut j.record
            && r.stamp == stamp
        {
            r.funds = Err(why.into());
            if !r.retry_used {
                r.retry_used = true;
                j.wake.notify_one();
            }
        } else if let Some((_, r)) = s
            .recent
            .iter_mut()
            .find(|(key, r)| *key == (wallet, t, stamp.generation) && r.stamp == stamp)
        {
            r.funds = Err(why.into());
        }
    }
    fn failed(&self, key: JobKey, id: u64, reason: String) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(j) = s.jobs.get_mut(&key)
            && j.id == id
        {
            j.last_error = Some(reason);
        }
    }
    fn identity(&self, key: JobKey) -> Option<layout::Identity> {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let r = s.jobs.get(&key)?.record.as_ref()?;
        r.funds.as_ref().ok().map(|(id, _, _)| *id)
    }
    fn publish(&self, key: JobKey, stamp: Stamp, funds: Funds) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(j) = s.jobs.get_mut(&key)
            && j.id == stamp.job
        {
            let retry_used = j
                .record
                .as_ref()
                .is_some_and(|r| r.stamp == stamp && r.retry_used);
            j.last_error = None;
            j.record = Some(Record {
                stamp,
                funds,
                retry_used,
            });
        }
    }
    fn binding(&self, t: Address, generation: u64, hash: B256) -> Arc<Binding> {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let slot = s
            .deployments
            .entry(t)
            .or_insert_with(|| ((generation, hash), Default::default()));
        if slot.0 != (generation, hash) {
            *slot = ((generation, hash), Default::default());
        }
        slot.1.clone()
    }
}
#[cfg(test)]
#[path = "presell_warm_tests.rs"]
mod tests;
