//! Время жизни фоновой работы определяется числом живых обращений к конкретной продаже.
use super::*;
impl State {
    pub(super) fn admit(&mut self, key: JobKey) -> Admission {
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
    pub(super) fn release(&mut self, key: JobKey, id: u64) {
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
                    if self.jobs.is_empty() {
                        self.deployments.clear();
                    }
                }
            }
        }
    }
}
