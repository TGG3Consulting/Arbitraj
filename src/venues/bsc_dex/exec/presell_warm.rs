//! Только выбранная предпродажа держит интерес к паре кошелёк/токен. При
//! отмене или выходе из предпроверки последний владелец отменяет фоновое чтение.
mod deployment;
mod lifecycle;
mod worker;
use super::presell_layout as layout;
use alloy_primitives::{Address, B256, Bytes, U256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, OnceCell};
pub(in crate::venues::bsc_dex) use worker::candidate;

/// Профиль существующего адаптера BSC. Номер фактической сети проверяется по его настройке.
pub(super) fn applies(ctx: &crate::venues::bsc_dex::ctx::Ctx) -> bool {
    ctx.place.id == crate::venues::bsc_dex::ctx::BSC_ID
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    job: u64,
    generation: u64,
    number: u64,
    pub(super) hash: B256,
}
type Funds = Result<(layout::Identity, layout::BalanceField, U256), String>;
type JobKey = (Address, Address, u64);
#[derive(Default)]
struct Binding {
    code: OnceCell<Bytes>,
    state: OnceCell<layout::Account>,
}
type CodeCells = HashMap<Address, Arc<Binding>>;
struct Record {
    stamp: Stamp,
    funds: Funds,
    retry_used: bool,
    negative: Option<layout::Negative>,
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
    block: Option<(u64, B256)>,
    deployments: CodeCells,
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
impl Cache {
    pub(super) fn read(
        &self,
        wallet: Address,
        t: Address,
        n: u64,
        generation: u64,
    ) -> Result<(Stamp, layout::BalanceField, U256), String> {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let key = (wallet, t, generation);
        let job = s.jobs.get(&key);
        let record = job
            .and_then(|j| j.record.as_ref())
            .or_else(|| s.recent.iter().find(|(k, _)| *k == key).map(|(_, r)| r));
        let reason = || job.and_then(|j| j.last_error.clone());
        let result = match record {
            Some(r) if r.stamp.number == n => {
                r.funds.clone().map(|(_, field, b)| (r.stamp, field, b))
            }
            Some(_) => {
                Err(reason().unwrap_or_else(|| "фоновая проверка относится к другому блоку".into()))
            }
            None => {
                Err(reason().unwrap_or_else(|| "фоновая проверка баланса ещё не готова".into()))
            }
        };
        if result.is_ok() {
            s.hits = s.hits.saturating_add(1)
        } else {
            s.misses = s.misses.saturating_add(1)
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
    fn identity(&self, key: JobKey) -> Option<(layout::Identity, layout::BalanceField)> {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let r = s.jobs.get(&key)?.record.as_ref()?;
        r.funds.as_ref().ok().map(|(id, field, _)| (*id, *field))
    }
    fn negative(&self, key: JobKey) -> Option<layout::Negative> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.jobs.get(&key)?.record.as_ref()?.negative.clone()
    }
    fn publish(&self, key: JobKey, stamp: Stamp, funds: Funds, negative: Option<layout::Negative>) {
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
                negative,
            });
        }
    }
}
#[cfg(test)]
#[path = "presell_warm_tests.rs"]
mod tests;
