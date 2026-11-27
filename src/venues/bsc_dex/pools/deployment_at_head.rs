//! Share verified deployments between markets at an identical canonical block and operator address/hash settings.
use alloy_primitives::{Address, B256};
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::venues::bsc_dex) struct Key {
    pub block: B256,
    pub contracts: [(Address, B256); 2],
}
type Slot<K, T> = (K, Arc<OnceCell<T>>);
pub(in crate::venues::bsc_dex) struct Cache<T = (), K = Key>(Mutex<Option<Slot<K, T>>>);
impl<T, K> Default for Cache<T, K> {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}
impl<T, K: PartialEq> Cache<T, K> {
    pub(super) fn at(&self, key: K) -> Arc<OnceCell<T>> {
        let mut current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((existing, cell)) = &*current
            && *existing == key
        {
            return cell.clone();
        }
        let cell = Arc::new(OnceCell::new());
        *current = Some((key, cell.clone()));
        cell
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shares_concurrent_reads_and_switches_on_reorg_or_profile_change() {
        let cache = Cache::<u32, (u8, u8)>::default();
        let a = cache.at((1, 7));
        let b = cache.at((1, 7));
        assert!(Arc::ptr_eq(&a, &b));
        assert!(
            a.get_or_try_init(|| async { Err::<u32, _>("partial read") })
                .await
                .is_err()
        );
        assert_eq!(b.get_or_init(|| async { 9 }).await, &9);
        assert_eq!(a.get(), Some(&9));
        assert!(cache.at((2, 7)).get().is_none());
        assert!(cache.at((1, 8)).get().is_none());
    }
}
