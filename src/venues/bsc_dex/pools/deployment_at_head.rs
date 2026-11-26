//! Share verified deployments between markets at an identical canonical block and operator address/hash settings.
use alloy_primitives::{Address, B256};
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Key {
    pub block: B256,
    pub contracts: [(Address, B256); 2],
}
type Slot = (Key, Arc<OnceCell<()>>);
#[derive(Default)]
pub(in crate::venues::bsc_dex) struct Cache(Mutex<Option<Slot>>);
impl Cache {
    pub(super) fn at(&self, key: Key) -> Arc<OnceCell<()>> {
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
