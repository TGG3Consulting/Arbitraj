//! Файл: core/registry_sync.rs
//! Что делает: держит справочник сетей биржи свежим (шаг 1 ТЗ): coin_networks через шлюз с частотой
//! venue.status_poll_ms (у BingX нет WebSocket для статусов ввода и вывода, решение 24), сразу —
//! по «толчку» (аномальный спред, ошибка вывода или депозита). Замечает, что у монеты открылся
//! или закрылся ввод или вывод, и уведомляет (событие venue.transfer_status): по монете или одной сводкой
//! при массовой смене (transfer_status.rs).
//! Кто вызывает: core/engine.rs — одна задача на каждую биржу.
//! От чего зависит: engine.rs, wait.rs, gateway (coin_networks), settings.
//! Настройки: venue.enabled, venue.status_poll_ms, venue.status_bulk_min (по площадке); сопоставление нативной
//! монеты с обёрткой (решение 62) — native.rs.

use std::collections::HashMap;

use crate::core::types::{CoinNetworkInfo, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, rd, wr};
use super::transfer_status;
use super::wait::{Pacer, Unit};

/// Включена ли площадка (venue.enabled). Не задано — выключена: значений по умолчанию нет.
pub fn enabled(core: &Core, venue: &VenueId) -> bool {
    core.inner
        .deps
        .settings
        .flag(
            "venue.enabled",
            At {
                venue: Some(&venue.0),
                coin: None,
            },
        )
        .unwrap_or(false)
}

/// Задача обновления сетей одной биржи. Работает, пока жива система. Ошибок не возвращает:
/// сбой пишется в журнал (повтор той же ошибки — один раз), старый справочник остаётся.
pub async fn run(core: Core, venue: VenueId) {
    let deps = core.inner.deps.clone();
    let mut pacer = Pacer::new("venue.status_poll_ms", Unit::Ms, Some(venue.0.clone()));
    let mut last_err: Option<String> = None;
    loop {
        if enabled(&core, &venue) {
            let res = match deps.gw.get(&venue).await {
                Ok(v) => v.coin_networks().await,
                Err(e) => Err(e),
            };
            match res {
                Ok(list) => {
                    last_err = None;
                    // Нативная монета (BNB без адреса) ↔ обёртка WBNB по настройке DEX — решение 62 (native.rs).
                    let list = super::native::map(&core, &venue, list);
                    report_changes(&core, &venue, &list).await;
                    wr(&core.inner.networks).insert(venue.clone(), list);
                    core.request_rebuild();
                }
                Err(e) if last_err.as_deref() != Some(e.message.as_str()) => {
                    deps.log(
                        Level::Warn,
                        "core.registry",
                        Some(&venue.0),
                        format!("Справочник сетей не обновлён: {}", e.message),
                        None,
                    )
                    .await;
                    last_err = Some(e.message);
                }
                Err(_) => {}
            }
        }
        pacer.pause(&deps, &core.inner.kick_networks).await;
    }
}

/// Сравнивает статусы ввода и вывода по сети BSC (у записи есть адрес контракта) со старыми и уведомляет:
/// по монете или одной сводкой при массовой смене (venue.status_bulk_min, transfer_status.rs).
/// Первая загрузка — без уведомлений. Ошибок нет.
async fn report_changes(core: &Core, venue: &VenueId, new: &[CoinNetworkInfo]) {
    let old: HashMap<(String, String), (bool, bool)> = {
        let nets = rd(&core.inner.networks);
        let Some(list) = nets.get(venue) else { return };
        list.iter()
            .map(|i| {
                (
                    (i.asset.ticker.clone(), i.network.clone()),
                    (i.deposit_enabled, i.withdraw_enabled),
                )
            })
            .collect()
    };
    let changes = transfer_status::diff(&old, new);
    if changes.is_empty() {
        return;
    }
    let deps = &core.inner.deps;
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    let min = deps.settings.integer("venue.status_bulk_min", at).ok();
    if transfer_status::bulk(changes.len(), min) {
        let (text, data) = transfer_status::summary(&venue.0, &changes);
        deps.log(
            Level::Warn,
            "core.transfer_status",
            Some(&venue.0),
            &text,
            Some(data),
        )
        .await;
        deps.notify("venue.transfer_status", Some(&venue.0), &text)
            .await;
        return;
    }
    for c in &changes {
        let text = transfer_status::one(&venue.0, c);
        deps.log(
            Level::Info,
            "core.transfer_status",
            Some(&venue.0),
            &text,
            None,
        )
        .await;
        deps.notify("venue.transfer_status", Some(&venue.0), &text)
            .await;
    }
}
