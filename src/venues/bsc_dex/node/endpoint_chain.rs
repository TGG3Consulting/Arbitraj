//! Подтверждения физической сети конечной HTTP-точки Base/Ethereum.
//! Один ответ действует до ошибки, проверки подключения или изменения конфигурации.
//! Старый параллельный ответ не восстанавливает сброшенное подтверждение.
use alloy_primitives::B256;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
pub(super) type Endpoint = (B256, u64);
#[derive(Default)]
struct Entry {
    settings: u64,
    epoch: u64,
    confirmed: bool,
}
#[derive(Default, Clone)]
pub(super) struct Endpoints(Arc<Mutex<HashMap<Endpoint, Entry>>>);
impl Endpoints {
    pub fn check(&self, key: Endpoint, settings: u64, refresh: bool) -> Option<u64> {
        let mut entries = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let e = entries.entry(key).or_default();
        if e.settings != settings {
            e.settings = settings;
            e.epoch = e.epoch.wrapping_add(1);
            e.confirmed = false;
        }
        (refresh || !e.confirmed).then_some(e.epoch)
    }
    pub fn forget(&self, key: Endpoint) {
        if let Some(e) = self
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&key)
        {
            e.confirmed = false;
            e.epoch = e.epoch.wrapping_add(1);
        }
    }
    pub fn confirm(&self, key: Endpoint, settings: u64, epoch: u64) -> bool {
        let mut entries = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let Some(e) = entries
            .get_mut(&key)
            .filter(|e| e.epoch == epoch && e.settings == settings)
        else {
            return false;
        };
        e.confirmed = true;
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_current_generation_authorizes_reuse_of_the_same_endpoint() {
        let c = Endpoints::default();
        let key = (alloy_primitives::keccak256("public"), 8453);
        let first = c.check(key, 1, false).unwrap();
        assert!(c.confirm(key, 1, first));
        assert!(c.check(key, 1, false).is_none());
        assert!(c.check((key.0, 1), 1, false).is_some());
        assert_eq!(c.check(key, 1, true), Some(first));
        c.forget(key);
        let after_error = c.check(key, 1, false).unwrap();
        assert!(!c.confirm(key, 1, first));
        assert!(c.confirm(key, 1, after_error));
        let changed = c.check(key, 2, false).unwrap();
        assert!(!c.confirm(key, 1, after_error));
        assert!(c.confirm(key, 2, changed));
        assert!(c.check(key, 2, false).is_none());
        assert!(
            c.check(
                (alloy_primitives::keccak256("rotated-key-url"), 8453),
                2,
                false
            )
            .is_some()
        );
    }
}
