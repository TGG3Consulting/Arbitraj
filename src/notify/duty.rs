//! Дополнительный односторонний канал дежурного. Только снимки состояний, для которых нужен человек.
//! Ядро публикует уже известные цифры. Рассылку, повтор и учёт доставки ведёт отдельная задача.

mod delivery;
mod incident;
mod store;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::time::Duration;
use tokio::sync::{Notify, broadcast::error::RecvError};

use crate::core::deal::Deal;
use crate::core::engine::{Core, lk};
use crate::core::manual_halted::needs_review;
use crate::core::types::StuckAlert;
use crate::core::usdt_return_state::UsdtReturn;
use crate::storage::journal::Level;

use super::dispatch::Notifier;
use delivery::Delivery;
use incident::{Incident, Thresholds};

#[derive(Default)]
pub(crate) struct Bus {
    pending: std::sync::Mutex<BTreeMap<String, Incident>>,
    dirty: tokio::sync::Notify,
}

impl Bus {
    fn offer(&self, core: &Core, make: impl FnOnce() -> Incident) {
        let settings = &core.inner.deps.settings;
        let limits = Thresholds::read(settings);
        if limits.configured(settings) {
            let i = make();
            if limits.qualifies(&i) {
                lk(&self.pending).entry(i.key.clone()).or_insert(i);
                self.dirty.notify_one();
            }
        }
    }

    pub(crate) fn alert(&self, core: &Core, alert: &StuckAlert, deal: &Deal) {
        self.offer(core, || Incident::alert(alert, deal));
    }

    pub(crate) fn saved(&self, core: &Core, deal: &Deal) {
        if needs_review(deal) {
            self.offer(core, || Incident::halted(deal));
        }
        for part in deal.usdt_return.iter().chain(&deal.usdt_return_parts) {
            if let UsdtReturn::NotReturned {
                at, why, amount, ..
            } = part
            {
                self.offer(core, || Incident::usdt(deal, at, why, *amount));
            }
        }
    }
}

/// Первый запуск и изменение конфигурации: только неурегулированные позиции.
fn snapshot(core: &Core) -> Vec<Incident> {
    let mut open = lk(&core.inner.alerts)
        .values()
        .map(|(a, d)| Incident::alert(a, d))
        .collect::<Vec<_>>();
    for id in crate::core::unsettled::ids(core) {
        // Копируем только нужные сделки, текст строим после снятия замка учёта.
        let deal = {
            let deals = lk(&core.inner.deals);
            deals
                .get(&id)
                .filter(|d| {
                    needs_review(d)
                        || d.usdt_return
                            .iter()
                            .chain(&d.usdt_return_parts)
                            .any(|p| matches!(p, UsdtReturn::NotReturned { .. }))
                })
                .cloned()
        };
        let Some(d) = deal else { continue };
        if needs_review(&d) {
            open.push(Incident::halted(&d));
        }
        for part in d.usdt_return.iter().chain(&d.usdt_return_parts) {
            if let UsdtReturn::NotReturned {
                at, why, amount, ..
            } = part
            {
                open.push(Incident::usdt(&d, at, why, *amount));
            }
        }
    }
    open
}

fn enqueue(pending: &mut BTreeMap<String, Incident>, incident: Incident, limits: &Thresholds) {
    if limits.qualifies(&incident) {
        // Сохраняем первую пересёкшую порог точку, даже если следующая попытка уже лучше.
        pending.entry(incident.key.clone()).or_insert(incident);
    }
}

/// Отправка не участвует в очереди прежних уведомлений, у дежурного нет приёма команд.
pub async fn run(core: Core, notifier: Notifier) {
    let added = Notify::new();
    tokio::join!(
        collect(&core, &notifier, &added),
        send(&core, &notifier, &added)
    );
}

async fn collect(core: &Core, n: &Notifier, added: &Notify) {
    let mut changes = n.settings.subscribe();
    let mut refresh = true;
    let mut pending = BTreeMap::new();
    loop {
        for (_, i) in std::mem::take(&mut *lk(&core.inner.duty.pending)) {
            pending.entry(i.key.clone()).or_insert(i);
        }
        if refresh {
            let limits = Thresholds::read(&n.settings);
            if limits.configured(&n.settings) {
                for i in snapshot(core) {
                    enqueue(&mut pending, i, &limits);
                }
            }
        }
        if !pending.is_empty() {
            match store::put(&core.inner.deps.db, &pending).await {
                Ok(()) => {
                    pending.clear();
                    added.notify_one();
                }
                Err(e) => {
                    n.log(
                        Level::Error,
                        None,
                        format!("Дежурный: очередь не сохранена — {e}"),
                    )
                    .await
                }
            }
        }
        refresh = false;
        tokio::select! {
            _ = core.inner.duty.dirty.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(30)), if !pending.is_empty() => {}
            changed = changes.recv() => match changed {
                Ok(c) => refresh = c.name.starts_with("telegram.duty_"),
                Err(RecvError::Lagged(_)) => refresh = true,
                Err(RecvError::Closed) => return,
            },
        }
    }
}

async fn send(core: &Core, n: &Notifier, added: &Notify) {
    let mut changes = n.settings.subscribe();
    let mut state = Delivery::default();
    loop {
        let retry = match store::load(&core.inner.deps.db).await {
            Ok(mut pending) if !pending.is_empty() => {
                let limits = Thresholds::read(&n.settings);
                let enabled = limits.configured(&n.settings);
                let keys: Vec<_> = pending.keys().cloned().collect();
                pending.retain(|_, i| enabled && limits.qualifies(i));
                if !pending.is_empty() {
                    state.flush(&core.inner.deps.db, n, &mut pending).await;
                }
                let done = keys
                    .into_iter()
                    .filter(|k| !pending.contains_key(k))
                    .collect();
                if let Err(e) = store::remove(&core.inner.deps.db, done).await {
                    n.log(
                        Level::Error,
                        None,
                        format!("Дежурный: не обновлена очередь — {e}"),
                    )
                    .await;
                    true
                } else {
                    !pending.is_empty()
                }
            }
            Ok(_) => false,
            Err(e) => {
                n.log(
                    Level::Error,
                    None,
                    format!("Дежурный: не прочитана очередь — {e}"),
                )
                .await;
                true
            }
        };
        loop {
            tokio::select! {
                _ = added.notified() => break,
                _ = tokio::time::sleep(Duration::from_secs(30)), if retry => break,
                changed = changes.recv() => match changed {
                    Ok(c) if c.name.starts_with("telegram.duty_") => break,
                    Ok(_) => {},
                    Err(RecvError::Lagged(_)) => break,
                    Err(RecvError::Closed) => return,
                }
            }
        }
    }
}
