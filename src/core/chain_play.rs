//! Файл: core/chain_play.rs
//! Что делает: решение 489 — «DEX в игре»: какие сети есть у ядра. Площадка DEX в игре — в реестре ядра вида DEX и
//! включена (venue.enabled = да) или её сеть держат идущие сделки (net_hold.rs: открытые / остановленные с монетами —
//! как поток по решению 196 — и закрытые с идущим возвратом USDT). Только площадка в игре даёт ядру свою сеть: привязка
//! записей сетей бирж (net_chain.rs others) и группы справочника (registry_chains::groups), отсюда монеты сети, их
//! подписка у бирж и USDT сети (rebuild.rs usdt_other). Выключенная площадка без сделок — сети для ядра нет. «DEX
//! реестра» (сеть площадки, сеть по умолчанию, учёт) — core/chain.rs dexes. Сеть, которую держат только сделки,
//! отмечается в Inner.net_held (net_hold::mark / unmark): смена отметки здесь — толчок перечитывания (net_kick.rs).
//! Кто вызывает: core/net_chain.rs (active_dexes), core/registry_chains.rs (in_play); тесты tests/core_net_chain_489*.rs.
//! От чего зависит: chain.rs (dexes), net_hold.rs, net_kick.rs, registry_sync.rs (venue.enabled), engine.rs (виды площадок).
//! Настройки: venue.enabled (по площадке DEX).

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;

use super::engine::{Core, rd};

/// Что делает: площадка DEX в игре (шапка). Принимает: ядро, площадку. Возвращает: да/нет. Ошибок нет (не задано — не
/// в игре). Замки: kinds, deals, net_held — коротко, по очереди, не вложенно.
pub fn in_play(core: &Core, v: &VenueId) -> bool {
    let dex = rd(&core.inner.kinds)
        .iter()
        .any(|(id, k)| id == v && *k == VenueKind::Dex);
    dex && live(core, v)
}

/// Что делает: площадки DEX в игре, по порядку реестра. Принимает: ядро. Ошибок нет.
pub fn active_dexes(core: &Core) -> Vec<VenueId> {
    super::chain::dexes(core)
        .into_iter()
        .filter(|v| live(core, v))
        .collect()
}

/// Включена ли площадка или её сеть держат сделки. Решение 489 (ревью 4): переход виден здесь раньше, чем сохранение
/// сделки дойдёт до net_hold::on_saved (ledger.rs: учёт памяти — запись SQLite — on_saved), — кто первым ставит или
/// снимает отметку (Inner.net_held), тот и толкает перечитывание справочника сетей (net_kick.rs): новая отметка —
/// толчок, снятая — толчок; уже стоящая — без толчка. Цикла нет: толчок пересборку не просит, а пересборка при
/// стоящей отметке не толкает. Ошибок нет.
fn live(core: &Core, v: &VenueId) -> bool {
    if super::registry_sync::enabled(core, v) {
        return true;
    }
    let held = super::net_hold::holds_net_any(core, v);
    let changed = if held {
        super::net_hold::mark(core, v)
    } else {
        super::net_hold::unmark(core, v)
    };
    if changed {
        super::net_kick::kick(core);
    }
    held
}
