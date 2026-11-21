//! Файл: core/net_kick.rs
//! Что делает: решения 431, 489 — толчок перечитывания справочника сетей бирж без потери. Раньше толчок был только
//! Notify::notify_waiters: будит задачи бирж (registry_sync.rs), которые сейчас ждут, а задача, занятая запросом
//! справочника, толчок теряла — и новая привязка к сети (например, повторный захват сети выключенной площадки DEX,
//! net_hold.rs) ждала планового опроса. Теперь толчок ещё и увеличивает поколение (Inner.networks_gen); задача биржи
//! запоминает поколение в начале прохода и перед паузой сверяет: толчок пришёл за время прохода — пауза пропускается,
//! справочник берётся снова (окно venue.networks_fresh_ms соблюдается, как раньше — registry_sync.rs fresh_left). Толчки
//! за один проход сливаются в один повтор — шторма нет.
//! Кто вызывает: kick — core/rebuild.rs (watch_settings), core/engine.rs (secrets_changed), core/evaluate.rs (аномалия),
//! core/feed_live.rs (подъём потока), core/net_hold.rs; generation, pause — core/registry_sync.rs.
//! От чего зависит: engine.rs (Inner.kick_networks, Inner.networks_gen), wait.rs (Pacer).
//! Настройки: через Pacer — venue.networks_poll_ms / venue.status_poll_ms.

use std::sync::atomic::Ordering;

use tokio::sync::Notify;

use super::engine::Core;
use super::wait::Pacer;

/// Что делает: толчок: поколение +1, затем будит ждущие задачи бирж. Принимает: ядро. Ошибок нет.
pub fn kick(core: &Core) {
    core.inner.networks_gen.fetch_add(1, Ordering::SeqCst);
    core.inner.kick_networks.notify_waiters();
}

/// Что делает: текущее поколение толчков. Принимает: ядро. Ошибок нет.
pub fn generation(core: &Core) -> u64 {
    core.inner.networks_gen.load(Ordering::SeqCst)
}

/// Что делает: пауза задачи справочника сетей до планового опроса (Pacer) или толчка. Ожидание толчка ставится ДО
/// сверки поколения: толчок после неё будит паузу, толчок до неё (после `seen`) пропускает паузу. Принимает: ядро,
/// интервал опроса, поколение в начале прохода. Ошибок нет.
pub async fn pause(core: &Core, pacer: &mut Pacer, seen: u64) {
    let woke = core.inner.kick_networks.notified();
    tokio::pin!(woke);
    woke.as_mut().enable();
    if generation(core) != seen {
        return;
    }
    // Pacer ждёт свой интервал (или настройку); толчок — через `woke`, своего Notify у Pacer здесь нет.
    let idle = Notify::new();
    tokio::select! {
        _ = pacer.pause(&core.inner.deps, &idle) => {}
        _ = &mut woke => {}
    }
}
