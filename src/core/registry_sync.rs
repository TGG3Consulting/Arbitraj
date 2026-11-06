//! Файл: core/registry_sync.rs
//! Что делает: держит справочник сетей биржи свежим (шаг 1 ТЗ): coin_networks через шлюз с частотой
//! venue.networks_poll_ms (решение 109; не задана — venue.status_poll_ms, как раньше; у BingX нет WebSocket для
//! статусов ввода и вывода, решение 24), сразу —
//! по «толчку» (аномальный спред, ошибка вывода или депозита). Замечает, что у монеты открылся
//! или закрылся ввод или вывод, и уведомляет (событие venue.transfer_status): по монете или одной сводкой
//! при массовой смене (transfer_status.rs).
//! Кто вызывает: core/engine.rs — одна задача на каждую биржу.
//! От чего зависит: engine.rs, wait.rs, gateway (coin_networks), settings.
//! Настройки: venue.enabled, venue.networks_poll_ms, venue.status_poll_ms, venue.status_bulk_min,
//! venue.status_confirm_polls (по площадке; сглаживание мигания статусов — status_debounce.rs, решение 112);
//! сопоставление нативной монеты с обёрткой (решение 62) — native.rs; venue.networks_fresh_ms — толчок внутри окна
//! свежести ждёт его конца (решение 129).
//! Решение 129: справочник без изменений (кроме времени получения) пересборку справочника монет не толкает.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    // Решение 109: справочник сетей MEXC (config/getall) — ~5.6 МБ на ответ; опрос с частотой опроса статусов переводов
    // (2 с) качал и разбирал его 30 раз в минуту и каждый раз пересобирал справочник монет. Своя частота — отдельно.
    let mut own = Pacer::new("venue.networks_poll_ms", Unit::Ms, Some(venue.0.clone()));
    let mut pacer = Pacer::new("venue.status_poll_ms", Unit::Ms, Some(venue.0.clone()));
    let mut last_err: Option<String> = None;
    // Решение 129: когда справочник получен последний раз — толчок (аномальный спред, ошибка вывода) внутри
    // venue.networks_fresh_ms ждёт конца окна, а не качает справочник заново (у MEXC ~5.6 МБ на ответ).
    let mut fetched: Option<Instant> = None;
    // Решение 112: смена статуса ввода/вывода принимается после venue.status_confirm_polls опросов подряд.
    let mut debounce = super::status_debounce::Debounce::default();
    loop {
        if enabled(&core, &venue) {
            if let Some(wait) = fresh_left(&core, &venue, fetched) {
                tokio::time::sleep(wait).await;
            }
            // Решение 183: окно свежести — только от удачного получения; сбой (например, секреты ещё закрыты) не
            // задерживает толчок после разблокировки.
            let started = Instant::now();
            let res = match deps.gw.get(&venue).await {
                Ok(v) => v.coin_networks().await,
                Err(e) => Err(e),
            };
            match res {
                Ok(list) => {
                    fetched = Some(started);
                    last_err = None;
                    // Нативная монета (BNB без адреса) ↔ обёртка WBNB по настройке DEX — решение 62 (native.rs).
                    let list = super::native::map(&core, &venue, list);
                    let list = debounce.apply(&core, &venue, list);
                    report_changes(&core, &venue, &list).await;
                    // Решение 129: тот же справочник (кроме времени получения) — без пересборки справочника монет.
                    let same = rd(&core.inner.networks)
                        .get(&venue)
                        .is_some_and(|old| same_lists(old, &list));
                    wr(&core.inner.networks).insert(venue.clone(), Arc::new(list));
                    if !same {
                        core.request_rebuild();
                    }
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
        let at = At {
            venue: Some(&venue.0),
            coin: None,
        };
        if deps.settings.integer("venue.networks_poll_ms", at).is_ok() {
            own.pause(&deps, &core.inner.kick_networks).await;
        } else {
            pacer.pause(&deps, &core.inner.kick_networks).await;
        }
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

/// Сколько ещё ждать до следующего получения справочника: venue.networks_fresh_ms задана и справочник получен моложе
/// неё (так бывает только по толчку — плановый опрос venue.networks_poll_ms реже). Не задана — None: толчок качает
/// справочник сразу, как до решения 129. Ошибок нет.
fn fresh_left(core: &Core, venue: &VenueId, fetched: Option<Instant>) -> Option<Duration> {
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    let fresh = core
        .inner
        .deps
        .settings
        .integer("venue.networks_fresh_ms", at)
        .ok()
        .map(Duration::from_millis)?;
    fresh
        .checked_sub(fetched?.elapsed())
        .filter(|d| !d.is_zero())
}

/// Одинаковы ли справочники по содержанию: все поля, кроме времени получения (updated_at). Ошибок нет.
pub fn same_lists(a: &[CoinNetworkInfo], b: &[CoinNetworkInfo]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.venue == y.venue
                && x.asset == y.asset
                && x.network == y.network
                && x.deposit_enabled == y.deposit_enabled
                && x.withdraw_enabled == y.withdraw_enabled
                && x.withdraw_fee == y.withdraw_fee
                && x.withdraw_fee_pct == y.withdraw_fee_pct
                && x.withdraw_min == y.withdraw_min
                && x.withdraw_max == y.withdraw_max
                && x.deposit_min == y.deposit_min
                && x.deposit_confirmations == y.deposit_confirmations
                && x.withdraw_precision == y.withdraw_precision
                && x.withdraw_step == y.withdraw_step
        })
}
