//! Isolated ledger and empty exchange registry for offline core regressions.
use super::{deps::Deps, engine::Core};
use crate::gateway::registry::Registry;
use crate::notify::dispatch::Notifier;
use crate::settings::store::Store;
use crate::storage::{db::Db, journal::Journal};
use std::path::Path;
use tokio::sync::broadcast;

pub(super) async fn open(path: &Path) -> Core {
    let db = Db::open(&path.join("ledger.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(32);
    let (tx, _) = broadcast::channel(32);
    let journal = Journal::new(db.clone(), tx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: crate::secrets::Vault::locked(path.join("secrets.enc")),
        journal: journal.clone(),
    };
    Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 32,
        data_dir: path.into(),
    })
    .await
}
